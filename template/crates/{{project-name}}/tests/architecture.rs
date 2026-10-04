//! Architecture guard. The workspace is first described as plain data (`cargo metadata`, the
//! member manifests and their sources); the pure function [`check`] then compares that data
//! with the rules in `docs/architecture.md`. Each rule has a test that breaks it on purpose
//! and expects exactly that rule to be reported.
//!
//! Members are recognised by their library names (`svc_<role>`), so nothing here depends on
//! the project name.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cargo_metadata::{DependencyKind, MetadataCommand};

type TestResult = Result<(), Box<dyn Error>>;

/// The members each member may depend on through normal and build dependencies, by role.
const ALLOWED: &[(&str, &[&str])] = &[
    ("util", &[]),
    ("domain", &["util"]),
    ("config", &["util"]),
    ("runtime", &["util"]),
    ("telemetry", &["util"]),
    ("api", &["domain", "runtime", "util"]),
    ("infra", &["domain", "runtime", "util"]),
    (
        "app",
        &[
            "api",
            "infra",
            "runtime",
            "telemetry",
            "config",
            "domain",
            "util",
        ],
    ),
    ("test_utils", &["domain", "runtime", "util"]),
];
/// The member every member may use, and only as a dev-dependency.
const DEV_ONLY: &str = "test_utils";
/// Crates that bring an async runtime or networking.
const IO: &[&str] = &[
    "tokio",
    "mio",
    "async-std",
    "async-io",
    "smol",
    "socket2",
    "hyper",
    "h2",
    "axum",
    "tower-http",
    "reqwest",
    "tonic",
];
/// Crates that make up an HTTP stack.
const HTTP: &[&str] = &["hyper", "h2", "axum", "tower-http", "reqwest", "tonic"];
/// Crates that must not be in a member's runtime closure with default features, by role.
const CLOSURE_DENY: &[(&str, &[&str])] = &[
    ("util", IO),
    ("domain", IO),
    ("config", IO),
    ("runtime", HTTP),
    ("telemetry", HTTP),
];
/// A second error model: no member depends on these directly.
const BANNED: &[&str] = &["anyhow", "eyre", "color-eyre", "failure"];
/// Names of the upstream error type that only the vendored file uses: an error kind of this
/// project is an `ErrorKind`, and no retry decision is left open.
const UPSTREAM_ONLY: &[&str] = &["Custom", "CustomCode", "ReusedOnly", "new_str", "new_code"];
/// The vendored upstream files, relative to their crate: `pingora.rs` and its submodule.
const VENDORED: &str = "src/error/pingora";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Normal,
    Build,
    Dev,
}

#[derive(Clone, Debug)]
struct Dep {
    name: String,
    member: Option<String>,
    kind: Kind,
}

#[derive(Clone, Debug)]
struct Member {
    role: String,
    node: String,
    deps: Vec<Dep>,
    manifest: toml::Table,
    /// Every `.rs` file under `src`, as (path relative to the crate, text).
    sources: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct Node {
    name: String,
    proc_macro: bool,
    /// Packages reached through normal and build dependencies.
    runtime_deps: Vec<String>,
}

#[derive(Clone, Debug)]
struct Ws {
    members: Vec<Member>,
    graph: BTreeMap<String, Node>,
}

/// A broken rule: its name and what breaks it.
type Violation = (&'static str, String);

// ---- description: the only part that runs cargo or reads files ---------------------------

fn sources(dir: &Path, crate_dir: &Path, out: &mut Vec<(String, String)>) -> TestResult {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, crate_dir, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let rel = path
                .strip_prefix(crate_dir)?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, fs::read_to_string(&path)?));
        }
    }
    Ok(())
}

fn describe() -> Result<Ws, Box<dyn Error>> {
    // Read at run time, so a shared target directory never describes another checkout.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("Cargo.toml");
    let meta = MetadataCommand::new().manifest_path(manifest).exec()?;
    let resolve = meta
        .resolve
        .as_ref()
        .ok_or("cargo metadata returned no graph")?;
    let mut graph = BTreeMap::new();
    for node in &resolve.nodes {
        let pkg = &meta[&node.id];
        let runtime = |k: &cargo_metadata::DepKindInfo| {
            matches!(k.kind, DependencyKind::Normal | DependencyKind::Build)
        };
        let node_data = Node {
            name: pkg.name.to_string(),
            proc_macro: pkg
                .targets
                .iter()
                .any(cargo_metadata::Target::is_proc_macro),
            runtime_deps: (node.deps.iter())
                .filter(|d| d.dep_kinds.iter().any(runtime))
                .map(|d| d.pkg.repr.clone())
                .collect(),
        };
        graph.insert(node.id.repr.clone(), node_data);
    }
    let workspace = meta.workspace_packages();
    let role_of = |pkg: &cargo_metadata::Package| {
        let lib = pkg.targets.iter().find(|t| t.is_lib());
        lib.map_or(pkg.name.to_string(), |t| {
            t.name.strip_prefix("svc_").unwrap_or(&t.name).to_string()
        })
    };
    let roles: BTreeMap<String, String> = (workspace.iter())
        .map(|p| (p.name.to_string(), role_of(p)))
        .collect();
    let mut members = Vec::new();
    for pkg in &workspace {
        let dir = pkg
            .manifest_path
            .parent()
            .ok_or("manifest without a directory")?;
        let mut files = Vec::new();
        sources(dir.join("src").as_std_path(), dir.as_std_path(), &mut files)?;
        let deps = (pkg.dependencies.iter())
            .map(|d| Dep {
                name: d.name.clone(),
                member: roles.get(&d.name).cloned(),
                kind: match d.kind {
                    DependencyKind::Build => Kind::Build,
                    DependencyKind::Development => Kind::Dev,
                    _ => Kind::Normal,
                },
            })
            .collect();
        members.push(Member {
            role: role_of(pkg),
            node: pkg.id.repr.clone(),
            deps,
            manifest: fs::read_to_string(&pkg.manifest_path)?.parse()?,
            sources: files,
        });
    }
    Ok(Ws { members, graph })
}

/// The description of the real workspace, computed once per test process.
fn workspace() -> Result<Ws, Box<dyn Error>> {
    static WS: OnceLock<Result<Ws, String>> = OnceLock::new();
    let ws = WS.get_or_init(|| describe().map_err(|e| e.to_string()));
    ws.clone().map_err(Into::into)
}

// ---- check: a pure function of the description -------------------------------------------

/// Packages reachable from `start` at run time, each with the path to it. Proc-macros are
/// listed but not followed: their dependencies only run at compile time.
fn closure(graph: &BTreeMap<String, Node>, start: &str) -> Vec<(String, String)> {
    let mut seen = BTreeSet::from([start.to_string()]);
    let mut queue = vec![(start.to_string(), String::new())];
    let mut found = Vec::new();
    while let Some((id, path)) = queue.pop() {
        for dep_id in graph.get(&id).map_or(&[][..], |n| &n.runtime_deps) {
            let Some(dep) = graph.get(dep_id).filter(|_| seen.insert(dep_id.clone())) else {
                continue;
            };
            let dep_path = format!("{path} -> {}", dep.name);
            found.push((dep.name.clone(), dep_path.clone()));
            if !dep.proc_macro {
                queue.push((dep_id.clone(), dep_path));
            }
        }
    }
    found
}

fn dependency_tables(manifest: &toml::Table) -> Vec<&toml::Table> {
    const KEYS: [&str; 5] = [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "dev_dependencies",
        "build_dependencies",
    ];
    let targets = manifest.get("target").and_then(toml::Value::as_table);
    let scopes = std::iter::once(manifest).chain(
        targets
            .into_iter()
            .flat_map(|t| t.values().filter_map(toml::Value::as_table)),
    );
    scopes
        .flat_map(|scope| KEYS.iter().filter_map(|key| scope.get(*key)?.as_table()))
        .collect()
}

fn is_workspace(value: Option<&toml::Value>) -> bool {
    value
        .and_then(|v| v.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}

fn check(ws: &Ws) -> Vec<Violation> {
    let mut out = Vec::new();
    for m in &ws.members {
        let role = &m.role;
        let Some(allowed) = ALLOWED.iter().find(|(r, _)| r == role).map(|(_, a)| *a) else {
            out.push(("layers", format!("{role} is not a known member")));
            continue;
        };
        for dep in &m.deps {
            if BANNED.contains(&dep.name.as_str()) {
                out.push(("one-error-model", format!("{role} depends on {}", dep.name)));
            }
            match dep.member.as_deref() {
                Some(DEV_ONLY) if dep.kind != Kind::Dev => {
                    out.push(("dev-only", format!("{role} -> {DEV_ONLY} ({:?})", dep.kind)));
                }
                Some(to) if to != DEV_ONLY && !allowed.contains(&to) => {
                    out.push(("layers", format!("{role} -> {to} ({:?})", dep.kind)));
                }
                _ => {}
            }
        }
        let denied = CLOSURE_DENY
            .iter()
            .find(|(r, _)| r == role)
            .map_or(&[][..], |(_, d)| *d);
        for (name, path) in closure(&ws.graph, &m.node) {
            if denied.contains(&name.as_str()) {
                out.push(("closure", format!("{role}{path}")));
            }
        }
        for table in dependency_tables(&m.manifest) {
            for (name, _) in table.iter().filter(|(_, v)| !is_workspace(Some(v))) {
                let detail = format!("{role}: {name} lacks workspace = true");
                out.push(("workspace-versions", detail));
            }
        }
        if !is_workspace(m.manifest.get("lints")) {
            out.push((
                "workspace-lints",
                format!("{role} lacks [lints] workspace = true"),
            ));
        }
        for (path, text) in m
            .sources
            .iter()
            .filter(|(path, _)| !path.starts_with(VENDORED))
        {
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                let words = code.split(|c: char| !(c.is_alphanumeric() || c == '_'));
                let found = words.filter(|w| UPSTREAM_ONLY.contains(w));
                let found = found.chain(code.contains("ErrorType::new(").then_some("new"));
                for word in found {
                    out.push(("upstream-names", format!("{role} {path}:{}: {word}", n + 1)));
                }
            }
        }
    }
    out
}

// ---- tests ----------------------------------------------------------------------------------

/// The names of the rules reported after `change` is applied to the real workspace.
fn rules_after(
    change: impl FnOnce(&mut Ws) -> TestResult,
) -> Result<Vec<&'static str>, Box<dyn Error>> {
    let mut ws = workspace()?;
    change(&mut ws)?;
    let rules: BTreeSet<&'static str> = check(&ws).into_iter().map(|(rule, _)| rule).collect();
    Ok(rules.into_iter().collect())
}

fn member<'a>(ws: &'a mut Ws, role: &str) -> Result<&'a mut Member, Box<dyn Error>> {
    let found = ws.members.iter_mut().find(|m| m.role == role);
    found.ok_or_else(|| format!("no member {role}").into())
}

fn dep(name: &str, member: Option<&str>, kind: Kind) -> Dep {
    let member = member.map(str::to_string);
    Dep {
        name: name.to_string(),
        member,
        kind,
    }
}

/// Adds `start -> names[0] -> names[1] -> ...` to the graph with synthetic packages.
fn add_path(ws: &mut Ws, start: &str, names: &[(&str, bool)]) {
    let mut from = start.to_string();
    for (name, proc_macro) in names {
        let id = format!("synthetic:{name}");
        let node = Node {
            name: (*name).to_string(),
            proc_macro: *proc_macro,
            runtime_deps: vec![],
        };
        ws.graph.insert(id.clone(), node);
        if let Some(node) = ws.graph.get_mut(&from) {
            node.runtime_deps.push(id.clone());
        }
        from = id;
    }
}

#[test]
fn the_workspace_keeps_every_rule() -> TestResult {
    assert_eq!(check(&workspace()?), Vec::<Violation>::new());
    Ok(())
}

#[test]
fn an_upward_dependency_breaks_the_layers() -> TestResult {
    let rules = rules_after(|ws| {
        let m = member(ws, "domain")?;
        m.deps.push(dep("x-api", Some("api"), Kind::Normal));
        Ok(())
    })?;
    assert_eq!(rules, ["layers"]);
    Ok(())
}

#[test]
fn test_utils_is_only_a_dev_dependency() -> TestResult {
    let rules = rules_after(|ws| {
        let m = member(ws, "api")?;
        m.deps
            .push(dep("x-test-utils", Some(DEV_ONLY), Kind::Normal));
        Ok(())
    })?;
    assert_eq!(rules, ["dev-only"]);
    Ok(())
}

#[test]
fn the_domain_reaches_no_io_crate() -> TestResult {
    let rules = rules_after(|ws| {
        let node = member(ws, "domain")?.node.clone();
        add_path(ws, &node, &[("some-lib", false), ("tokio", false)]);
        Ok(())
    })?;
    assert_eq!(rules, ["closure"]);
    Ok(())
}

#[test]
fn proc_macros_are_not_followed() -> TestResult {
    let rules = rules_after(|ws| {
        let node = member(ws, "domain")?.node.clone();
        add_path(ws, &node, &[("some-derive", true), ("tokio", false)]);
        Ok(())
    })?;
    assert_eq!(rules, Vec::<&str>::new());
    Ok(())
}

#[test]
fn no_member_depends_on_a_second_error_model() -> TestResult {
    let rules = rules_after(|ws| {
        member(ws, "util")?
            .deps
            .push(dep("anyhow", None, Kind::Dev));
        Ok(())
    })?;
    assert_eq!(rules, ["one-error-model"]);
    Ok(())
}

#[test]
fn versions_are_declared_in_the_workspace() -> TestResult {
    let rules = rules_after(|ws| {
        let manifest = &mut member(ws, "util")?.manifest;
        manifest.insert(
            "dev_dependencies".into(),
            toml::toml! { serde = "1" }.into(),
        );
        Ok(())
    })?;
    assert_eq!(rules, ["workspace-versions"]);
    Ok(())
}

#[test]
fn lints_come_from_the_workspace() -> TestResult {
    let rules = rules_after(|ws| {
        member(ws, "util")?.manifest.remove("lints");
        Ok(())
    })?;
    assert_eq!(rules, ["workspace-lints"]);
    Ok(())
}

#[test]
fn upstream_names_stay_in_the_vendored_file() -> TestResult {
    let line = "let kind = ErrorType::Custom(\"Mine\");\n".to_string();
    let rules = rules_after(|ws| {
        let sources = &mut member(ws, "util")?.sources;
        sources.push((format!("{VENDORED}.rs"), line.clone()));
        sources.push(("src/notes.rs".to_string(), format!("// {line}")));
        Ok(())
    })?;
    assert_eq!(rules, Vec::<&str>::new());
    let rules = rules_after(|ws| {
        member(ws, "domain")?
            .sources
            .push(("src/mine.rs".to_string(), line));
        Ok(())
    })?;
    assert_eq!(rules, ["upstream-names"]);
    Ok(())
}
