//! 环境变量入口：位置（`ENV_PREFIX_CONFIG`）与值覆盖（`ENV_PREFIX_SECTION__KEY`）。
//!
//! 读取环境变量走 [`EnvSource`] 接缝：生产用 [`ProcessEnv`]，测试注入 [`MapEnv`]。
//! 这样测试之间不会通过进程环境互相干扰（同一进程里并行跑多个用例）。
//!
//! 值覆盖只认"带 `__` 分隔"的名字：`SVC_HTTP__BIND` → `http.bind`。
//! 形状匹配但键不存在 → 错误（拼错键即错误）；没有 `__` 的名字不归我们管（留给用户自己的
//! 环境变量与 `url_env` 指向的秘密变量）。

use std::collections::BTreeMap;

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

use crate::schema::FileConfig;

/// 环境变量前缀：由生成时的 `crate_name` 派生（大写、`_` 分隔），写进代码而不是靠猜。
pub const ENV_PREFIX: &str = "{{env_prefix}}";

/// 配置文件位置的环境变量名。
pub fn config_env_name() -> String {
    format!("{ENV_PREFIX}_CONFIG")
}

/// 环境变量读取接缝。
pub trait EnvSource: Send + Sync + 'static {
    /// 取一个变量的值；不存在返回 `None`。
    fn get(&self, name: &str) -> Option<String>;

    /// 取当前所有变量名（用于扫描覆盖键）。只需要名字，值再走 `get`。
    fn names(&self) -> Vec<String>;
}

/// 进程环境（生产路径）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn names(&self) -> Vec<String> {
        std::env::vars().map(|(name, _)| name).collect()
    }
}

/// 固定表环境（测试路径：不给进程环境留副作用）。
#[derive(Debug, Default, Clone)]
pub struct MapEnv {
    values: BTreeMap<String, String>,
}

impl MapEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }
}

impl EnvSource for MapEnv {
    fn get(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }

    fn names(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }
}

/// 一次覆盖应用的结果：真正写进树的路径（排序去重，便于日志与断言稳定）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OverlayApplied {
    pub paths: Vec<String>,
}

/// 把环境变量覆盖应用到配置树上。
///
/// `defaults` 是 schema 默认值的树：它同时是"允许的键集合"与"值该按什么类型解析"的参照。
/// 形状匹配但不在 `defaults` 里的键会被拒绝（一次列出全部拼错的键）。
pub(crate) fn apply_overlay(
    tree: &mut toml::Value,
    env: &dyn EnvSource,
    defaults: &toml::Value,
) -> Result<OverlayApplied, Error> {
    let prefix = format!("{ENV_PREFIX}_");
    let mut unknown = Vec::new();
    let mut applied = Vec::new();

    for name in env.names() {
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if !rest.contains("__") {
            // 没有层级分隔符：不是配置覆盖键（例如 ENV_PREFIX_CONFIG 或用户自己的变量）。
            continue;
        }
        let path = rest.to_ascii_lowercase().replace("__", ".");
        let Some(reference) = lookup(defaults, &path) else {
            unknown.push(format!("{name} → {path}"));
            continue;
        };
        let raw = env
            .get(&name)
            .ok_or_else(|| Error::config(format!("环境变量 `{name}` 在读取时消失了")))?;
        let value = parse_like(reference, &raw, &name)?;
        set(tree, &path, value);
        applied.push(path);
    }

    if !unknown.is_empty() {
        return Err(Error::config(format!(
            "这些环境变量看起来想覆盖配置，但对应的键不存在（拼错即错误）：{}",
            unknown.join("、")
        )));
    }

    applied.sort();
    applied.dedup();
    Ok(OverlayApplied { paths: applied })
}

fn lookup<'a>(mut value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    for segment in path.split('.') {
        value = value.get(segment)?;
    }
    Some(value)
}

fn set(tree: &mut toml::Value, path: &str, value: toml::Value) {
    let mut segments = path.split('.').peekable();
    let mut current = tree;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            if let toml::Value::Table(table) = current {
                table.insert(segment.to_owned(), value);
            }
            return;
        }
        let table = current.as_table_mut().expect("defaults 树保证中间层都是表");
        current = table
            .entry(segment.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    }
}

/// 按默认值的类型解析环境变量取值：类型对不上就是错误，不做隐式转换。
fn parse_like(reference: &toml::Value, raw: &str, name: &str) -> Result<toml::Value, Error> {
    match reference {
        toml::Value::String(_) => Ok(toml::Value::String(raw.to_owned())),
        toml::Value::Integer(_) => {
            raw.trim()
                .parse::<i64>()
                .map(toml::Value::Integer)
                .map_err(|_| {
                    Error::new(
                        ErrorKind::Config,
                        format!("环境变量 `{name}` 期望整数，收到 `{raw}`"),
                    )
                })
        }
        toml::Value::Boolean(_) => raw
            .trim()
            .parse::<bool>()
            .map(toml::Value::Boolean)
            .map_err(|_| {
                Error::new(
                    ErrorKind::Config,
                    format!("环境变量 `{name}` 期望布尔值（true/false），收到 `{raw}`"),
                )
            }),
        other => Err(Error::new(
            ErrorKind::Config,
            format!("环境变量 `{name}` 对应的键类型不支持覆盖：{other}"),
        )),
    }
}

/// schema 默认值的树：所有键都出现，用于覆盖键白名单与类型参照。
pub(crate) fn defaults_tree() -> toml::Value {
    let defaults = FileConfig::default();
    toml::Value::try_from(defaults).expect("FileConfig 默认值必须能序列化成 TOML")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(names: &[(&str, &str)]) -> Result<(toml::Value, OverlayApplied), Error> {
        let mut env = MapEnv::new();
        for (name, value) in names {
            env = env.with(*name, *value);
        }
        let mut tree = defaults_tree();
        let applied = apply_overlay(&mut tree, &env, &defaults_tree())?;
        Ok((tree, applied))
    }

    #[test]
    fn applies_keys_with_double_underscore_only() {
        let (tree, applied) = overlay(&[
            (&format!("{ENV_PREFIX}_HTTP__BIND"), "127.0.0.1:8081"),
            (&format!("{ENV_PREFIX}_LOG__FILTER"), "debug"),
            (&format!("{ENV_PREFIX}_CONFIG"), "/tmp/other.toml"),
            (&format!("{ENV_PREFIX}_STORAGE_URL"), "sqlite:ignored.db"),
        ])
        .expect("覆盖应当成功");
        assert_eq!(tree["http"]["bind"].as_str(), Some("127.0.0.1:8081"));
        assert_eq!(tree["log"]["filter"].as_str(), Some("debug"));
        assert_eq!(
            tree["storage"]["url"].as_str(),
            Some("sqlite:data/service.db?mode=rwc")
        );
        assert_eq!(applied.paths, vec!["http.bind", "log.filter"]);
    }

    #[test]
    fn unknown_key_in_overlay_shape_is_an_error() {
        let err = overlay(&[(&format!("{ENV_PREFIX}_HTTP__BIND_PORT"), "8081")])
            .expect_err("拼错的键必须报错");
        let rendered = err.to_string();
        assert!(rendered.contains("HTTP__BIND_PORT"), "{rendered}");
        assert!(rendered.contains("http.bind_port"), "{rendered}");
    }

    #[test]
    fn integer_override_must_parse() {
        let err = overlay(&[(
            &format!("{ENV_PREFIX}_SUPERVISOR__RESTART_BACKOFF_MS"),
            "soon",
        )])
        .expect_err("类型不匹配必须报错");
        assert!(err.to_string().contains("期望整数"), "{err}");

        let (tree, _) = overlay(&[(
            &format!("{ENV_PREFIX}_SUPERVISOR__RESTART_BACKOFF_MS"),
            "900",
        )])
        .expect("整数覆盖应当成功");
        assert_eq!(
            tree["supervisor"]["restart_backoff_ms"].as_integer(),
            Some(900)
        );
    }
}
