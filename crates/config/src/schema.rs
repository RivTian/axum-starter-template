//! 配置 schema：文件形态（[`FileConfig`]）→ 生效形态（[`Config`]）。
//!
//! 两层的分工：
//! - `FileConfig` 是文件里的原始形态：每个字段可省略（有默认）、未知键即错误；
//! - `Config` 是生效形态：类型化（`SocketAddr`、`Secret<String>`）、路径已从锚点派生、越界已钳位。
//!
//! 从文件到 `Config` 的转换只发生在 [`Config::from_file`]，管线与重载共用它——这是"启动与热重载
//! 不漂移"的落点。
//!
//! 注意：`Config` 的 `Serialize`/`Deserialize` 是给重载的 diff/回滚用的内部接缝；
//! 从文件到配置一律走管线，不要用反序列化绕开校验与路径派生。

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use {{crate_prefix_snake}}_core::{Error, ErrorKind};

use crate::anchor::Anchor;
use crate::env::EnvSource;
use crate::secret::{self, Secret};

/// `[storage].busy_timeout_ms` 的钳位下界。
pub const MIN_BUSY_TIMEOUT_MS: u64 = 100;
/// `[storage].busy_timeout_ms` 的钳位上界。
pub const MAX_BUSY_TIMEOUT_MS: u64 = 300_000;

/// 文件里的原始形态：所有段可省略，未知键即错误。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub log: FileLog,
    pub supervisor: FileSupervisor,
    pub http: FileHttp,
    pub storage: FileStorage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileLog {
    pub filter: String,
}

impl Default for FileLog {
    fn default() -> Self {
        Self {
            filter: "info".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileSupervisor {
    pub restart_backoff_ms: u64,
    pub restart_backoff_cap_ms: u64,
}

impl Default for FileSupervisor {
    fn default() -> Self {
        Self {
            restart_backoff_ms: 500,
            restart_backoff_cap_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileHttp {
    /// 监听地址的文本形态；解析（带错误定位）在 [`Config::from_file`] 里做。
    pub bind: String,
}

impl Default for FileHttp {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:0".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileStorage {
    pub url: String,
    pub url_env: String,
    pub url_file: String,
    pub busy_timeout_ms: u64,
}

impl Default for FileStorage {
    fn default() -> Self {
        Self {
            url: "sqlite:data/service.db?mode=rwc".to_owned(),
            url_env: String::new(),
            url_file: String::new(),
            busy_timeout_ms: 5_000,
        }
    }
}

/// 生效形态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub log: LogConfig,
    pub supervisor: SupervisorConfig,
    pub http: HttpConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    pub filter: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    pub restart_backoff_ms: u64,
    pub restart_backoff_cap_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub url: Secret<String>,
    pub busy_timeout_ms: u64,
}

/// 管线产生的非致命提示：钳位、遮蔽、默认模板落盘失败。装配层负责把它们打成日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// 相关的配置叶子路径（或 `config` 这样的伪路径）。
    pub path: String,
    pub kind: NoticeKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// 取值越界，已钳到保守值。
    Clamped,
    /// 多层敏感信息同时存在，高优先级把低的遮蔽了。
    ShadowedSecretSource,
    /// 缺配置文件且默认模板写不进锚点（只读目录），继续用内存里的默认值。
    DefaultNotWritten,
    /// 重载时遇到未登记档位的叶子路径：按 cold 处理（回滚 + 待重启）并报这里。
    UnclassifiedPath,
}

impl NoticeKind {
    /// 稳定的短名：用于日志字段与测试断言。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clamped => "clamped",
            Self::ShadowedSecretSource => "shadowed-secret-source",
            Self::DefaultNotWritten => "default-not-written",
            Self::UnclassifiedPath => "unclassified-path",
        }
    }
}

impl Config {
    /// 文件形态 → 生效形态：补全、校验（只拦必然失败）、解析敏感信息、从锚点派生路径、钳位。
    pub(crate) fn from_file(
        file: FileConfig,
        anchor: &Anchor,
        env: &dyn EnvSource,
    ) -> Result<(Self, Vec<Notice>), Error> {
        let mut notices = Vec::new();

        let filter = {
            let trimmed = file.log.filter.trim();
            if trimmed.is_empty() {
                "info".to_owned()
            } else {
                trimmed.to_owned()
            }
        };

        let backoff = file.supervisor.restart_backoff_ms;
        if backoff == 0 {
            return Err(Error::new(
                ErrorKind::Config,
                "supervisor.restart_backoff_ms 不能为 0：那会把任务重启变成忙等",
            ));
        }
        let mut cap = file.supervisor.restart_backoff_cap_ms;
        if cap < backoff {
            notices.push(Notice {
                path: "supervisor.restart_backoff_cap_ms".to_owned(),
                kind: NoticeKind::Clamped,
                detail: format!("{cap} 小于 restart_backoff_ms={backoff}，已提升到 {backoff}"),
            });
            cap = backoff;
        }

        let bind: SocketAddr = file.http.bind.trim().parse().map_err(|_| {
            Error::new(
                ErrorKind::Config,
                format!(
                    "http.bind 不是合法的 IP:端口：`{}`（例如 127.0.0.1:0）",
                    file.http.bind
                ),
            )
        })?;

        let requested_timeout = file.storage.busy_timeout_ms;
        let busy_timeout_ms = requested_timeout.clamp(MIN_BUSY_TIMEOUT_MS, MAX_BUSY_TIMEOUT_MS);
        if busy_timeout_ms != requested_timeout {
            notices.push(Notice {
                path: "storage.busy_timeout_ms".to_owned(),
                kind: NoticeKind::Clamped,
                detail: format!(
                    "{requested_timeout} 超出 [{MIN_BUSY_TIMEOUT_MS}, {MAX_BUSY_TIMEOUT_MS}]，已钳位到 {busy_timeout_ms}"
                ),
            });
        }

        let (url, mut secret_notices) = secret::resolve(&file.storage, anchor, env)?;
        notices.append(&mut secret_notices);
        let url = Secret::new(anchor.rewrite_sqlite_url(url.expose()).to_owned());

        let config = Self {
            log: LogConfig { filter },
            supervisor: SupervisorConfig {
                restart_backoff_ms: backoff,
                restart_backoff_cap_ms: cap,
            },
            http: HttpConfig { bind },
            storage: StorageConfig {
                url,
                busy_timeout_ms,
            },
        };
        Ok((config, notices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::MapEnv;

    fn anchor() -> Anchor {
        Anchor::from_dir("/srv/app")
    }

    fn file_with(bind: &str, backoff: u64, cap: u64, busy: u64) -> FileConfig {
        let mut file = FileConfig::default();
        file.http.bind = bind.to_owned();
        file.supervisor.restart_backoff_ms = backoff;
        file.supervisor.restart_backoff_cap_ms = cap;
        file.storage.busy_timeout_ms = busy;
        file
    }

    #[test]
    fn rejects_values_that_cannot_run() {
        let err = Config::from_file(
            file_with("not-an-address", 500, 30_000, 5_000),
            &anchor(),
            &MapEnv::new(),
        )
        .expect_err("非法的 bind 必须拒绝");
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("http.bind"), "{err}");

        let err = Config::from_file(
            file_with("127.0.0.1:0", 0, 30_000, 5_000),
            &anchor(),
            &MapEnv::new(),
        )
        .expect_err("0 退避必须拒绝");
        assert!(err.to_string().contains("restart_backoff_ms"), "{err}");
    }

    #[test]
    fn clamps_out_of_range_with_notice() {
        let (config, notices) = Config::from_file(
            file_with("127.0.0.1:8080", 500, 200, 50),
            &anchor(),
            &MapEnv::new(),
        )
        .expect("越界只钳位");
        assert_eq!(config.storage.busy_timeout_ms, MIN_BUSY_TIMEOUT_MS);
        assert_eq!(config.supervisor.restart_backoff_cap_ms, 500);
        assert_eq!(notices.len(), 2);
        assert!(
            notices
                .iter()
                .all(|notice| notice.kind == NoticeKind::Clamped)
        );
    }

    #[test]
    fn sqlite_path_is_anchored() {
        let (config, _) = Config::from_file(FileConfig::default(), &anchor(), &MapEnv::new())
            .expect("默认配置必须可用");
        assert_eq!(
            config.storage.url.expose(),
            "sqlite:/srv/app/data/service.db?mode=rwc"
        );
        assert_eq!(config.http.bind.to_string(), "127.0.0.1:0");
    }
}
