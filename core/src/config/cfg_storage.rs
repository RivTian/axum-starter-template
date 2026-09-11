//! 存储子配置（`[storage]` 段）
//!
//! 全部字段带 serde 默认值：配置文件缺失 `[storage]` 段时等价于
//! 「SQLite + `<安装根>/data/` 下的库文件」，零配置可用。
//!
//! # 敏感信息
//!
//! PostgreSQL 密码按 `password_env` > `password_file` > `password` 顺序取值；
//! [`PostgresConfig`] 手写 `Debug`，密码一律渲染为 `***`，防止连带日志泄漏。
//!
//! # 定义在 core 而消费在 storage
//!
//! 配置类型属于「所有 crate 共享、不含常驻任务」的内容，放 core 才能让装配层
//! 在不依赖 storage 的前提下谈论配置；连接池与 SQL 一律不在此。
//!
//! # 整段不可热变更（冻结语义）
//!
//! 连接池与迁移在启动时一次成型，运行中无法重建；`ConfigStore` 的白名单把
//! `storage.` 前缀整体归入 `requires_restart`。

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::clamp::clamp_field;
use super::error::ConfigError;
use super::path_util::resolve_relative;
use crate::util::DATA_DIR;

// ── 顶层 ─────────────────────────────────────────────────────────────────────

/// 存储后端类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// 嵌入式 SQLite（默认，单机零依赖）
    #[default]
    Sqlite,
    /// PostgreSQL（多实例 / 高配部署）
    Postgres,
}

/// `[storage]` 段完整配置
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    pub sqlite: SqliteConfig,
    pub postgres: PostgresConfig,
}

impl StorageConfig {
    /// 相对路径一律相对**安装根**解析；SQLite 路径留空时在这里推导缺省：
    /// `<root>/data/{{crate_name}}.db`，与 `config/` 同级而不是嵌在它里面。
    /// 推导放在这一步而不是后端里，因为只有加载管线知道安装根在哪。
    pub(super) fn resolve_paths(&mut self, root: &Path) {
        if self.sqlite.path.is_empty() {
            self.sqlite.path = root
                .join(DATA_DIR)
                .join(SqliteConfig::DEFAULT_FILE_NAME)
                .to_string_lossy()
                .into_owned();
        } else {
            resolve_relative(&mut self.sqlite.path, root);
        }
        resolve_relative(&mut self.postgres.password_file, root);
    }

    /// 硬错误：取值非法到「继续跑必然失败」的程度，提前到加载期报错。
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        match self.postgres.sslmode.as_str() {
            "disable" | "prefer" | "require" => Ok(()),
            other => Err(ConfigError::Invalid(format!(
                "[storage.postgres].sslmode = {other:?} 无效；可选值为 disable | prefer | require"
            ))),
        }
    }

    /// 软越界：钳位 + warn。
    pub(super) fn sanitize(&mut self) {
        clamp_field(
            &mut self.sqlite.read_pool_size,
            1,
            64,
            "storage.sqlite",
            "read_pool_size",
        );
        clamp_field(
            &mut self.sqlite.busy_timeout_ms,
            100,
            600_000,
            "storage.sqlite",
            "busy_timeout_ms",
        );
        clamp_field(
            &mut self.postgres.pool_size,
            1,
            256,
            "storage.postgres",
            "pool_size",
        );
        clamp_field(
            &mut self.postgres.connect_timeout_ms,
            100,
            600_000,
            "storage.postgres",
            "connect_timeout_ms",
        );
    }
}

// ── SQLite ───────────────────────────────────────────────────────────────────

/// `[storage.sqlite]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SqliteConfig {
    /// 数据库文件路径；留空 ⇒ `<安装根>/data/{{crate_name}}.db`。
    /// 相对路径相对安装根解析。
    pub path: String,
    /// 写锁等待上限（毫秒）；单写者池下写操作天然排队，此值仅为最后一道保险
    pub busy_timeout_ms: u64,
    /// 只读连接池大小（写池固定为 1，见 storage crate 的单写者纪律）
    pub read_pool_size: u32,
}

impl SqliteConfig {
    pub const DEFAULT_FILE_NAME: &'static str = "{{crate_name}}.db";
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            busy_timeout_ms: 5_000,
            read_pool_size: 4,
        }
    }
}

// ── PostgreSQL ───────────────────────────────────────────────────────────────

/// `[storage.postgres]`
///
/// 密码不写进配置文件是默认姿势：`password_env` 指向环境变量名，
/// `password_file` 指向 secret 文件（容器编排挂载），`password` 仅限实验环境。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PostgresConfig {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    /// 密码来源 1（最高优先级）：环境变量名
    pub password_env: String,
    /// 密码来源 2：secret 文件路径（读取后 trim）；相对路径相对安装根解析
    pub password_file: String,
    /// 密码来源 3（仅限实验环境；非空时启动打 warn，但绝不输出取值）
    pub password: String,
    /// TLS 模式：disable | prefer | require
    pub sslmode: String,
    pub pool_size: u32,
    pub connect_timeout_ms: u64,
}

impl PostgresConfig {
    /// 解析密码：`password_env` > `password_file` > `password`；均未提供 ⇒ `None`。
    ///
    /// 返回 `String` 而非借用：调用方拿到后立即交给连接选项，不进任何长生命周期结构。
    pub fn resolve_password(&self) -> std::io::Result<Option<String>> {
        if !self.password_env.is_empty()
            && let Ok(value) = std::env::var(&self.password_env)
            && !value.is_empty()
        {
            return Ok(Some(value));
        }
        if !self.password_file.is_empty() {
            let content = std::fs::read_to_string(&self.password_file)?;
            return Ok(Some(content.trim().to_string()));
        }
        if !self.password.is_empty() {
            return Ok(Some(self.password.clone()));
        }
        Ok(None)
    }
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 5432,
            dbname: "{{crate_name}}".into(),
            user: "{{crate_name}}".into(),
            password_env: "{{env_prefix}}_PG_PASSWORD".into(),
            password_file: String::new(),
            password: String::new(),
            sslmode: "prefer".into(),
            pool_size: 5,
            connect_timeout_ms: 5_000,
        }
    }
}

/// 手写 `Debug`：`password` 渲染为 `***`。
///
/// 派生 `Debug` 会让密码随任何一次 `tracing::debug!(?cfg)` 或 panic 信息落盘，
/// 而配置结构被顺手打印是极难在 review 中拦住的——在类型上关死这条路。
impl std::fmt::Debug for PostgresConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("dbname", &self.dbname)
            .field("user", &self.user)
            .field("password_env", &self.password_env)
            .field("password_file", &self.password_file)
            .field("password", &"***")
            .field("sslmode", &self.sslmode)
            .field("pool_size", &self.pool_size)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_yields_sqlite_defaults() {
        let cfg: StorageConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.backend, StorageBackend::Sqlite);
        assert!(cfg.sqlite.path.is_empty());
        assert_eq!(cfg.sqlite.read_pool_size, 4);
        assert_eq!(cfg.postgres.port, 5432);
        assert_eq!(cfg.postgres.sslmode, "prefer");
    }

    #[test]
    fn backend_parses_lowercase() {
        let cfg: StorageConfig = toml::from_str(r#"backend = "postgres""#).unwrap();
        assert_eq!(cfg.backend, StorageBackend::Postgres);
    }

    /// 空路径推导到 `<root>/data/` 下；非空相对路径按安装根解析（不带 `data/`）
    #[test]
    fn resolve_paths_derives_default_sqlite_file() {
        let mut cfg = StorageConfig::default();
        cfg.resolve_paths(Path::new("/etc/svc"));
        assert_eq!(
            cfg.sqlite.path,
            format!("/etc/svc/{DATA_DIR}/{}", SqliteConfig::DEFAULT_FILE_NAME)
        );

        let mut rel = StorageConfig::default();
        rel.sqlite.path = "x/y.db".into();
        rel.resolve_paths(Path::new("/etc/svc"));
        assert_eq!(rel.sqlite.path, "/etc/svc/x/y.db");
    }

    #[test]
    fn invalid_sslmode_is_a_hard_error() {
        let mut cfg = StorageConfig::default();
        cfg.postgres.sslmode = "verify-full".into();
        assert!(matches!(cfg.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn sanitize_clamps_pool_sizes() {
        let mut cfg = StorageConfig::default();
        cfg.sqlite.read_pool_size = 0;
        cfg.postgres.pool_size = 0;
        cfg.sanitize();
        assert_eq!(cfg.sqlite.read_pool_size, 1);
        assert_eq!(cfg.postgres.pool_size, 1);
    }

    /// 密码优先级：env > file > 明文；Debug 不泄漏
    #[test]
    fn password_resolution_and_masking() {
        let cfg = PostgresConfig {
            password_env: String::new(),
            password: "plain".into(),
            ..PostgresConfig::default()
        };
        assert_eq!(cfg.resolve_password().unwrap(), Some("plain".into()));
        let debug = format!("{cfg:?}");
        assert!(debug.contains("***"), "{debug}");
        assert!(!debug.contains("plain"), "{debug}");

        let none = PostgresConfig {
            password_env: String::new(),
            ..PostgresConfig::default()
        };
        assert_eq!(none.resolve_password().unwrap(), None);
    }
}
