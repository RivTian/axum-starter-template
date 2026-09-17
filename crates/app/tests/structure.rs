//! 结构纪律测试：依赖邻接表、依赖为什么、窄门面、零全局态、库不装 subscriber。
//!
//! 这些检查是"机械校验"而不是"文档表述"：越界、漏登记、忘了写理由都会在这里红。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // crates/app -> crates -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("工作区根目录")
        .to_path_buf()
}

/// 本 crate 的包名（`<prefix>-app`），由此推出全部成员目录与包名。
fn prefix() -> String {
    env!("CARGO_PKG_NAME")
        .strip_suffix("-app")
        .expect("app crate 的包名必须以 -app 结尾")
        .to_owned()
}

fn members() -> Vec<String> {
    ["core", "config", "runtime", "storage", "http", "app"]
        .iter()
        .map(|suffix| format!("crates/{suffix}"))
        .collect()
}

/// 邻接表：成员（按后缀）→ 允许的兄弟依赖（按后缀）。这是唯一的事实来源。
const ADJACENCY: &[(&str, &[&str])] = &[
    ("core", &[]),
    ("config", &["core"]),
    ("runtime", &["core"]),
    ("storage", &["core"]),
    ("http", &["core"]),
    ("app", &["core", "config", "runtime", "storage", "http"]),
];

fn read_manifest(path: &Path) -> toml::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("读取 {} 失败：{err}", path.display()));
    toml::from_str(&text).unwrap_or_else(|err| panic!("解析 {} 失败：{err}", path.display()))
}

/// 一个成员的三张依赖表合并：key（依赖键）→ 值。
fn dependency_entries(manifest: &toml::Value) -> BTreeMap<String, toml::Value> {
    let mut entries = BTreeMap::new();
    for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(section) = manifest.get(table).and_then(toml::Value::as_table) else {
            continue;
        };
        for (key, value) in section {
            entries.insert(key.clone(), value.clone());
        }
    }
    entries
}

fn suffix_of(key: &str, prefix: &str) -> Option<String> {
    key.strip_prefix(&format!("{prefix}-")).map(str::to_owned)
}

#[test]
fn sibling_edges_match_adjacency_table() {
    let prefix = prefix();
    let root = workspace_root();
    for member in members() {
        let suffix = member.rsplit('/').next().expect("目录名");
        let manifest = read_manifest(&root.join(&member).join("Cargo.toml"));
        let actual: BTreeSet<String> = dependency_entries(&manifest)
            .keys()
            .filter_map(|key| suffix_of(key, &prefix))
            .collect();
        let expected: BTreeSet<String> = ADJACENCY
            .iter()
            .find(|(name, _)| *name == suffix)
            .unwrap_or_else(|| panic!("邻接表里没有成员 `{suffix}`"))
            .1
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(
            actual, expected,
            "`{member}` 的兄弟依赖边与邻接表不一致（多一条、少一条都红）"
        );
    }
}

#[test]
fn core_is_a_leaf() {
    let prefix = prefix();
    let root = workspace_root();
    let manifest = read_manifest(&root.join("crates/core/Cargo.toml"));
    let siblings: Vec<String> = dependency_entries(&manifest)
        .keys()
        .filter_map(|key| suffix_of(key, &prefix))
        .collect();
    assert!(
        siblings.is_empty(),
        "core 不能依赖任何兄弟 crate：{siblings:?}"
    );
}

#[test]
fn member_deps_are_workspace_inherited() {
    let root = workspace_root();
    for member in members() {
        let manifest = read_manifest(&root.join(&member).join("Cargo.toml"));
        for (key, value) in dependency_entries(&manifest) {
            let inherited = value
                .get("workspace")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            assert!(
                inherited,
                "`{member}` 的依赖 `{key}` 必须写成 workspace = true（依赖只允许在根声明一次）"
            );
        }
    }
}

#[test]
fn workspace_deps_have_reasons_and_are_used() {
    let root = workspace_root();
    let manifest = read_manifest(&root.join("Cargo.toml"));
    let declared = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
        .expect("根 Cargo.toml 必须有 [workspace.dependencies]");

    let mut used = BTreeSet::new();
    for member in members() {
        let member_manifest = read_manifest(&root.join(&member).join("Cargo.toml"));
        used.extend(dependency_entries(&member_manifest).into_keys());
    }

    let raw = std::fs::read_to_string(root.join("Cargo.toml")).expect("根 Cargo.toml");
    for key in declared.keys() {
        assert!(
            used.contains(key),
            "`{key}` 声明在根依赖表里但没有任何成员使用：没消费者的依赖不该在表里"
        );
        let line = raw
            .lines()
            .find(|line| {
                line.trim_start().starts_with(&format!("\"{key}\" = "))
                    || line.trim_start().starts_with(&format!("{key} = "))
            })
            .unwrap_or_else(|| panic!("找不到依赖 `{key}` 的声明行"));
        assert!(
            line.contains('#'),
            "依赖 `{key}` 缺少行内理由注释（每条依赖旁一句「为什么」）"
        );
    }
}

#[test]
fn lib_facades_are_narrow() {
    let root = workspace_root();
    for member in members() {
        let path = root.join(&member).join("src/lib.rs");
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("读 lib.rs");
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            let allowed = trimmed.is_empty()
                || trimmed.starts_with("//")
                || trimmed.starts_with("#![")
                || (trimmed.starts_with("mod ") && trimmed.ends_with(';'))
                || (trimmed.starts_with("pub use ") && trimmed.ends_with(';'));
            assert!(
                allowed,
                "{}:{} 的门面写法不窄：`{trimmed}`（只允许 mod 声明、显式 pub use、文档注释与属性）",
                path.display(),
                index + 1
            );
            if trimmed.starts_with("pub use ") && trimmed.contains("::*") {
                assert!(
                    trimmed.contains("// 例外:"),
                    "{}:{} 出现了通配 re-export，必须写成显式列表或加 `// 例外: 理由`",
                    path.display(),
                    index + 1
                );
            }
        }
    }
}

#[test]
fn no_global_state_in_crates() {
    let root = workspace_root();
    let forbidden = [
        "OnceLock",
        "LazyLock",
        "lazy_static",
        "once_cell",
        "static mut",
    ];
    for file in rust_sources(&root) {
        // 跳过本文件：它自己带着这些禁词作为模式串。
        if file.ends_with("tests/structure.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("读源文件");
        for needle in forbidden {
            assert!(
                !text.contains(needle),
                "{} 里出现了 `{needle}`：零业务全局态是硬约束（装配层参数注入）",
                file.display()
            );
        }
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("static ") {
                for bad in ["Mutex", "RwLock", "Cell", "RefCell"] {
                    assert!(
                        !trimmed.contains(bad),
                        "{}:{} 出现了可变的全局状态：`{trimmed}`",
                        file.display(),
                        index + 1
                    );
                }
            }
        }
    }
}

#[test]
fn no_subscriber_init_in_libs() {
    let root = workspace_root();
    let forbidden = ["set_global_default", "try_init", "tracing_subscriber"];
    for member in ["core", "config", "runtime", "storage", "http"] {
        for file in rust_sources(&root.join("crates").join(member)) {
            let text = std::fs::read_to_string(&file).expect("读源文件");
            for needle in forbidden {
                assert!(
                    !text.contains(needle),
                    "{} 里出现了 `{needle}`：库 crate 只发事件，装 subscriber 是装配层的事",
                    file.display()
                );
            }
        }
    }
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rust(root, &mut out);
    out
}

fn collect_rust(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}
