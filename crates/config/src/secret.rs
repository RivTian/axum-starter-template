//! 敏感取值：三档来源（`*_env` > `*_file` > 明文）与防泄漏的 `Debug`。

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use {{crate_prefix_snake}}_core::{Error, ErrorKind};

use crate::anchor::Anchor;
use crate::env::EnvSource;
use crate::schema::{FileStorage, Notice, NoticeKind};

/// 敏感取值。`Debug` 永远渲染 `***`，取值必须显式 [`Secret::expose`]。
///
/// 故意不实现 `Display`：这样它没法被 `format!("{value}")` 或 `%value` 顺手打进日志。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(value)
    }

    /// 显式取值：调用点必须自己清楚"这里会碰到明文"。
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl Serialize for Secret<String> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret<String> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Secret)
    }
}

/// 解析 `[storage]` 的敏感取值：`url_env > url_file > url`。
pub(crate) fn resolve(
    storage: &FileStorage,
    anchor: &Anchor,
    env: &dyn EnvSource,
) -> Result<(Secret<String>, Vec<Notice>), Error> {
    let literal = storage.url.trim();
    let env_name = storage.url_env.trim();
    let file_path = storage.url_file.trim();
    let mut notices = Vec::new();

    if !env_name.is_empty() {
        let value = env.get(env_name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Config,
                    format!(
                        "storage.url_env 指向的环境变量 `{env_name}` 不存在或为空（要么取消 url_env，要么把它设好）"
                    ),
                )
            })?;
        if !literal.is_empty() || !file_path.is_empty() {
            notices.push(Notice {
                path: "storage.url".to_owned(),
                kind: NoticeKind::ShadowedSecretSource,
                detail: "url_env 生效，url/url_file 被遮蔽（这是固定优先级，不是错误）".to_owned(),
            });
        }
        return Ok((Secret::new(value), notices));
    }

    if !file_path.is_empty() {
        let path = anchor.join(file_path);
        let raw = std::fs::read_to_string(&path).map_err(|err| {
            Error::with_source(
                ErrorKind::Config,
                format!("storage.url_file 指向的文件读不到：`{}`", path.display()),
                err,
            )
        })?;
        let value = raw.trim_end_matches(['\n', '\r']).to_owned();
        if value.is_empty() {
            return Err(Error::config(format!(
                "storage.url_file 指向的文件是空的：`{}`",
                path.display()
            )));
        }
        if !literal.is_empty() {
            notices.push(Notice {
                path: "storage.url".to_owned(),
                kind: NoticeKind::ShadowedSecretSource,
                detail: "url_file 生效，url 被遮蔽（这是固定优先级，不是错误）".to_owned(),
            });
        }
        return Ok((Secret::new(value), notices));
    }

    if literal.is_empty() {
        return Err(Error::config(
            "storage.url 是空的，且没有 url_env / url_file 兜底：数据库地址必须来自某处",
        ));
    }
    Ok((Secret::new(literal.to_owned()), notices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::MapEnv;

    fn anchor() -> Anchor {
        Anchor::from_dir("/srv/app")
    }

    fn default_storage() -> FileStorage {
        FileStorage::default()
    }

    #[test]
    fn debug_never_renders_the_value() {
        let secret = Secret::new("sqlite:/srv/app/very-secret.db".to_owned());
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, "Secret(***)");
        assert!(!rendered.contains("very-secret"));
    }

    #[test]
    fn env_wins_over_file_and_literal() {
        let mut storage = default_storage();
        storage.url = "sqlite:literal.db".to_owned();
        storage.url_env = "TEST_SERVICE_URL".to_owned();
        let env = MapEnv::new().with("TEST_SERVICE_URL", "sqlite:from-env.db");
        let (secret, notices) = resolve(&storage, &anchor(), &env).expect("env 应优先");
        assert_eq!(secret.expose(), "sqlite:from-env.db");
        assert!(
            notices
                .iter()
                .any(|notice| notice.kind == NoticeKind::ShadowedSecretSource)
        );
    }

    #[test]
    fn file_wins_over_literal_and_is_anchored() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("url.txt"), "sqlite:from-file.db\n").expect("写文件");
        let mut storage = default_storage();
        storage.url_file = "url.txt".to_owned();
        let (secret, _) = resolve(&storage, &Anchor::from_dir(dir.path()), &MapEnv::new())
            .expect("url_file 应可读");
        assert_eq!(secret.expose(), "sqlite:from-file.db");
    }

    #[test]
    fn missing_env_or_file_is_an_error() {
        let mut storage = default_storage();
        storage.url_env = "TEST_SERVICE_MISSING_URL".to_owned();
        let err = resolve(&storage, &anchor(), &MapEnv::new()).expect_err("缺失必须报错");
        assert!(err.to_string().contains("不存在或为空"), "{err}");

        let mut storage = default_storage();
        storage.url_file = "nope.txt".to_owned();
        let err = resolve(&storage, &anchor(), &MapEnv::new()).expect_err("缺失必须报错");
        assert!(err.to_string().contains("读不到"), "{err}");
    }

    #[test]
    fn empty_literal_without_fallback_is_an_error() {
        let mut storage = default_storage();
        storage.url = "  ".to_owned();
        let err = resolve(&storage, &anchor(), &MapEnv::new()).expect_err("空 url 必须报错");
        assert!(err.to_string().contains("storage.url"), "{err}");
    }
}
