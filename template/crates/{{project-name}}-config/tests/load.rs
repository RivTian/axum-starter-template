//! The loader with a small schema of its own: sources and their order, every problem at once,
//! the input shapes that went wrong before, and the `check-config` table.

use std::error::Error;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use svc_config::load::{Inputs, Loaded, load};
use svc_config::source::Source;
use svc_config::table::{overrides, render, rows};

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Schema {
    server: Server,
    log: Log,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Server {
    #[serde(deserialize_with = "svc_util::de::socket_addr")]
    addr: SocketAddr,
    #[serde(
        serialize_with = "svc_util::duration::serialize",
        deserialize_with = "svc_util::de::duration::<_, 1, 300>"
    )]
    timeout: Duration,
    #[serde(deserialize_with = "svc_util::de::integer::<_, 1, 65536>")]
    limit: usize,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
            timeout: Duration::from_secs(15),
            limit: 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Log {
    #[serde(deserialize_with = "svc_util::de::non_empty_text")]
    filter: String,
}

impl Default for Log {
    fn default() -> Self {
        Log {
            filter: "info".to_string(),
        }
    }
}

const ALIASES: &[(&str, &str)] = &[("log.filter", "RUST_LOG")];

fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect()
}

fn load_with(
    file: Option<&Path>,
    env: &[(OsString, OsString)],
    cli: &[(&'static str, &'static str, String)],
) -> Result<Loaded<Schema>, Vec<String>> {
    let inputs = Inputs {
        file,
        env,
        prefix: "APP",
        aliases: ALIASES,
        cli,
    };
    load::<Schema>(&inputs)
        .map_err(|report| report.problems().iter().map(ToString::to_string).collect())
}

/// A file with this text, removed when the test ends.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str, text: &str) -> Result<Self, Box<dyn Error>> {
        let path = std::env::temp_dir().join(format!("svc-config-{}-{name}", std::process::id()));
        std::fs::write(&path, text)?;
        Ok(TempFile(path))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

#[test]
fn later_sources_win_and_each_key_knows_its_source() -> TestResult {
    let file = TempFile::new(
        "order.toml",
        "[server]\nlimit = 10\ntimeout = \"20s\"\n[log]\nfilter = \"warn\"\n",
    )?;
    let vars = env(&[("RUST_LOG", "debug"), ("APP_SERVER__TIMEOUT", "30s")]);
    let cli = [("server.addr", "--addr", "0.0.0.0:9000".to_string())];
    let loaded = load_with(Some(&file.0), &vars, &cli).map_err(|p| p.join("\n"))?;
    assert_eq!(loaded.config.server.limit, 10);
    assert_eq!(loaded.config.server.timeout, Duration::from_secs(30));
    assert_eq!(loaded.config.log.filter, "debug");
    assert_eq!(loaded.config.server.addr.port(), 9000);
    let sources: Vec<String> = loaded
        .sources
        .iter()
        .map(|(k, s)| format!("{k}: {s}"))
        .collect();
    assert_eq!(
        sources,
        [
            "log.filter: env RUST_LOG",
            "server.addr: cli --addr",
            format!("server.limit: file {}", file.0.display()).as_str(),
            "server.timeout: env APP_SERVER__TIMEOUT",
        ]
    );
    Ok(())
}

#[test]
fn every_problem_is_reported_at_once_sorted_by_key() {
    let vars = env(&[("APP_SERVER__TIMEOUT", "5 seconds"), ("APP_NOPE__X", "1")]);
    let cli = [("server.limit", "--limit", "0".to_string())];
    assert_eq!(
        load_with(None, &vars, &cli).err(),
        Some(vec![
            "invalid configuration: nope.x (env APP_NOPE__X): unknown key".to_string(),
            "invalid configuration: server.limit (cli --limit): must be between 1 and 65536, got \"0\"".to_string(),
            "invalid configuration: server.timeout (env APP_SERVER__TIMEOUT): expected a duration like \"5s\" or \"250ms\", got \"5 seconds\"".to_string(),
        ])
    );
}

#[test]
fn prefixed_variables_without_a_double_underscore_are_not_configuration() {
    // Kubernetes adds these for a Service named like the project; `APP_CONFIG` names the file.
    let vars = env(&[
        ("APP_SERVICE_HOST", "10.0.0.1"),
        ("APP_PORT", "tcp://x"),
        ("APP_CONFIG", "a.toml"),
    ]);
    assert!(load_with(None, &vars, &[]).is_ok());
}

#[test]
fn an_empty_alias_counts_as_unset_and_a_section_is_not_a_key() {
    let loaded = load_with(None, &env(&[("RUST_LOG", "")]), &[]);
    assert_eq!(
        loaded.map(|l| l.config.log.filter).ok().as_deref(),
        Some("info")
    );
    assert_eq!(
        load_with(None, &env(&[("APP_SERVER", "x")]), &[]).err(),
        None,
        "a name without `__` after the prefix is ignored"
    );
    let problems = load_with(None, &env(&[("APP_LOG__FILTER", "")]), &[]).err();
    assert_eq!(
        problems,
        Some(vec![
            "invalid configuration: log.filter (env APP_LOG__FILTER): must not be empty"
                .to_string()
        ])
    );
}

#[test]
fn file_shapes_that_went_wrong_before() -> TestResult {
    let file = TempFile::new(
        "shapes.toml",
        "\"server.limit\" = 7\nserver = 1\n[server]\nlimit = 99999999999999999999999\n",
    )?;
    let problems = load_with(Some(&file.0), &[], &[]).err().unwrap_or_default();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("is not valid TOML at line"),
        "{problems:?}"
    );
    let file = TempFile::new(
        "keys.toml",
        "\"server.limit\" = 7\nlog = 1\n[server]\nlimit = \"99999999999999999999\"\n",
    )?;
    let shown = file.0.display().to_string();
    assert_eq!(
        load_with(Some(&file.0), &[], &[]).err(),
        Some(vec![
            format!("invalid configuration: \"server.limit\" (file {shown}): unknown key"),
            format!("invalid configuration: log (file {shown}): is a section, not a key"),
            format!(
                "invalid configuration: server.limit (file {shown}): must be between 1 and 65536, got \"99999999999999999999\""
            ),
        ])
    );
    Ok(())
}

#[test]
fn an_unreadable_file_is_one_problem() {
    let missing = Path::new("/nonexistent/app.toml");
    let problems = load_with(Some(missing), &[], &[]).err().unwrap_or_default();
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0]
            .starts_with("invalid configuration: file /nonexistent/app.toml cannot be read:")
    );
}

#[test]
fn a_value_that_is_not_utf8_is_a_problem_unless_a_later_source_replaces_it() {
    let bad = vec![(
        OsString::from("APP_LOG__FILTER"),
        OsString::from_vec(vec![b'x', 0xff]),
    )];
    assert_eq!(
        load_with(None, &bad, &[]).err(),
        Some(vec!["invalid configuration: log.filter (env APP_LOG__FILTER): expected UTF-8 text, got \"x\u{fffd}\"".to_string()])
    );
    let cli = [("log.filter", "--log-filter", "warn".to_string())];
    assert!(load_with(None, &bad, &cli).is_ok());
}

#[test]
fn the_table_counts_characters_and_escapes_control_characters() -> TestResult {
    let cli = [("log.filter", "--log-filter", "wärn\nfake  row".to_string())];
    let loaded = load_with(None, &[], &cli).map_err(|p| p.join("\n"))?;
    let table = render(&rows(&loaded));
    assert_eq!(
        table,
        concat!(
            "key             value              source\n",
            "log.filter      \"wärn\\nfake  row\"  cli --log-filter\n",
            "server.addr     127.0.0.1:8080     default\n",
            "server.limit    1024               default\n",
            "server.timeout  15s                default\n",
        )
    );
    assert_eq!(
        overrides(&rows(&loaded)),
        "log.filter=\"wärn\\nfake  row\" (cli --log-filter)"
    );
    assert_eq!(rows(&loaded)[1].source, Source::Default);
    Ok(())
}

#[test]
fn no_key_or_source_can_start_a_line_of_its_own() -> TestResult {
    let file = TempFile::new("forge.toml", "\"x\\ninvalid configuration: forged\" = 1\n")?;
    let vars = env(&[("APP_X__Y\nFORGED", "1")]);
    let problems = load_with(Some(&file.0), &vars, &[])
        .err()
        .unwrap_or_default();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().all(|p| !p.contains('\n')), "{problems:?}");
    let named = PathBuf::from(format!("{}\nlog.filter  debug  default", file.0.display()));
    let problems = load_with(Some(&named), &[], &[]).err().unwrap_or_default();
    assert!(problems.iter().all(|p| !p.contains('\n')), "{problems:?}");
    Ok(())
}
