//! 热度分档与"漏登记会红"的机制。
//!
//! 三档：
//! - 可热 `HOT_PATHS`（白名单）：改完立即生效（当前只有日志过滤器）；
//! - 半热 `SEMI_PATHS`（白名单）：写进共享句柄，下次使用时生效（当前是重启退避参数）；
//! - 不可热 `COLD_PREFIXES`（前缀列表，其余一律按 cold）：回滚为运行值并列入待重启清单。
//!
//! 关键设计：**分类按叶子路径，白名单显式，未命中的路径是 `Unknown`**。`Unknown` 在运行期按 cold
//! 处理（安全方向：回滚 + 报错级事件，绝不"假报已生效"）；测试
//! `every_leaf_path_is_explicitly_classified` 会把未登记的新配置字段直接变红——这就是
//! "冷段漏登记的判别方式"：不是"按面登记回调"，而是"每个叶子都必须被显式分类，否则测试红"。

use std::collections::BTreeSet;

/// 可热路径：改完立即生效。
pub const HOT_PATHS: &[&str] = &["log.filter"];

/// 半热路径：写进共享句柄，下次使用时生效。
pub const SEMI_PATHS: &[&str] = &[
    "supervisor.restart_backoff_ms",
    "supervisor.restart_backoff_cap_ms",
];

/// 不可热前缀：命中即 cold。
pub const COLD_PREFIXES: &[&str] = &["http.", "storage."];

/// 一个配置叶子的热度档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Hot,
    Semi,
    Cold,
    /// 没有登记过的路径：运行期按 cold 处理，测试里必须为红。
    Unknown,
}

/// 按叶子路径分类。
pub fn classify(path: &str) -> Tier {
    if HOT_PATHS.contains(&path) {
        Tier::Hot
    } else if SEMI_PATHS.contains(&path) {
        Tier::Semi
    } else if COLD_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
        Tier::Cold
    } else {
        Tier::Unknown
    }
}

/// 枚举一棵 TOML 树里的叶子路径（`a.b` 形式，排序）。
pub fn leaf_paths(value: &toml::Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out.sort();
    out
}

fn walk(value: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let path = join(prefix, key);
                walk(child, &path, out);
            }
        }
        _ => out.push(prefix.to_owned()),
    }
}

/// 两棵树在叶子级别不同的路径（排序）。值相同不算不同；一边缺键算不同。
pub(crate) fn different_leaves(a: &toml::Value, b: &toml::Value) -> Vec<String> {
    let mut out = Vec::new();
    diff_walk(a, b, "", &mut out);
    out.sort();
    out
}

fn diff_walk(a: &toml::Value, b: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    match (a.as_table(), b.as_table()) {
        (Some(a_table), Some(b_table)) => {
            let keys: BTreeSet<&String> = a_table.keys().chain(b_table.keys()).collect();
            for key in keys {
                let path = join(prefix, key);
                match (a_table.get(key), b_table.get(key)) {
                    (Some(a_child), Some(b_child)) => diff_walk(a_child, b_child, &path, out),
                    _ => out.push(path),
                }
            }
        }
        _ => {
            if a != b {
                out.push(prefix.to_owned());
            }
        }
    }
}

/// 把 `from` 里 `path` 的叶子值复制到 `to` 的同一路径。路径缺失返回 `false`。
pub(crate) fn copy_leaf(from: &toml::Value, to: &mut toml::Value, path: &str) -> bool {
    let Some(value) = read_path(from, path) else {
        return false;
    };
    write_path(to, path, value.clone());
    true
}

fn read_path<'a>(mut value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    for segment in path.split('.') {
        value = value.get(segment)?;
    }
    Some(value)
}

fn write_path(tree: &mut toml::Value, path: &str, value: toml::Value) {
    let mut segments = path.split('.').peekable();
    let mut current = tree;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            if let toml::Value::Table(table) = current {
                table.insert(segment.to_owned(), value);
            }
            return;
        }
        let table = current
            .as_table_mut()
            .expect("两侧都是同一 schema 的树，中间层必须是表");
        current = table
            .entry(segment.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    }
}

fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_semi_and_cold_are_each_reachable() {
        assert_eq!(classify("log.filter"), Tier::Hot);
        assert_eq!(classify("supervisor.restart_backoff_ms"), Tier::Semi);
        assert_eq!(classify("http.bind"), Tier::Cold);
        assert_eq!(classify("storage.url"), Tier::Cold);
        assert_eq!(classify("storage.busy_timeout_ms"), Tier::Cold);
    }

    #[test]
    fn unlisted_paths_are_unknown() {
        assert_eq!(classify("http.timeout_ms"), Tier::Cold);
        assert_eq!(classify("metrics.interval_ms"), Tier::Unknown);
        assert_eq!(classify("log"), Tier::Unknown);
    }

    #[test]
    fn leaf_paths_walks_tables_only() {
        let tree: toml::Value = toml::from_str(
            r#"
            [a]
            x = 1
            [a.b]
            y = "two"
        "#,
        )
        .expect("合法 TOML");
        assert_eq!(leaf_paths(&tree), vec!["a.b.y", "a.x"]);
    }

    #[test]
    fn different_leaves_ignores_equal_values() {
        let a: toml::Value = toml::from_str("[x]\ny = 1\nz = 2\n").expect("合法 TOML");
        let b: toml::Value = toml::from_str("[x]\ny = 1\nz = 3\n").expect("合法 TOML");
        assert_eq!(different_leaves(&a, &b), vec!["x.z"]);
    }

    #[test]
    fn copy_leaf_restores_a_value() {
        let running: toml::Value = toml::from_str("[x]\ny = 1\n").expect("合法 TOML");
        let mut candidate: toml::Value = toml::from_str("[x]\ny = 9\n").expect("合法 TOML");
        assert!(copy_leaf(&running, &mut candidate, "x.y"));
        assert_eq!(candidate["x"]["y"].as_integer(), Some(1));
        assert!(!copy_leaf(&running, &mut candidate, "x.missing"));
    }
}
