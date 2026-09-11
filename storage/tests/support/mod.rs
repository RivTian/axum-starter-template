//! 双后端集成测试共用的脚手架：起库、PG 门控
//!
//! 每个集成测试文件以 `mod support;` 引入本模块（cargo 不把 `tests/` 的子目录
//! 当独立测试目标）。SQLite 侧一用例一库（`CARGO_TARGET_TMPDIR` 下按
//! `{suite}/{case}` 独占目录，可并行）；PG 侧由 `{{env_prefix}}_TEST_PG_HOST`
//! 门控、共真实例。
#![allow(dead_code)] // 各测试文件只取所需

use std::path::PathBuf;

use {{crate_prefix_snake}}_core::config::{PostgresConfig, SqliteConfig};
use {{crate_prefix_snake}}_core::config::{StorageBackend, StorageConfig};

/// PG 门控变量的公共前缀。项目名不直接写进表达式：那样这些行的宽度会随项目名
/// 长短在 rustfmt 的 100 列上下翻转，模板的 fmt 门禁就只对某些名字成立
pub const PG_ENV_PREFIX: &str = "{{env_prefix}}_TEST_PG";

/// 一用例一库的 SQLite 配置；返回 (库文件路径, 配置)
pub fn sqlite_config(suite: &str, case: &str) -> (PathBuf, StorageConfig) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(suite)
        .join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建测试目录");
    let db = dir.join("test.db");
    let cfg = StorageConfig {
        backend: StorageBackend::Sqlite,
        sqlite: SqliteConfig {
            path: db.to_string_lossy().into_owned(),
            ..SqliteConfig::default()
        },
        ..StorageConfig::default()
    };
    (db, cfg)
}

/// 从环境变量组装 PG 配置；主变量缺失即视为「本机无 PG」→ `None`（调用方跳过）。
///
/// 主变量**设了却连不上**不在这里兜：调用方会照常 `init_storage` 并红掉——
/// 静默跳过会让「PG 侧没跑过」看起来和「PG 侧全绿」一模一样。
pub fn pg_config() -> Option<StorageConfig> {
    let var = |name: &str| std::env::var(format!("{PG_ENV_PREFIX}_{name}")).ok();
    let host = var("HOST")?;
    let mut postgres = PostgresConfig {
        host,
        password_env: format!("{PG_ENV_PREFIX}_PASSWORD"),
        sslmode: "disable".into(),
        ..PostgresConfig::default()
    };
    if let Some(port) = var("PORT").and_then(|p| p.parse().ok()) {
        postgres.port = port;
    }
    if let Some(dbname) = var("DBNAME") {
        postgres.dbname = dbname;
    }
    if let Some(user) = var("USER") {
        postgres.user = user;
    }
    Some(StorageConfig {
        backend: StorageBackend::Postgres,
        postgres,
        ..StorageConfig::default()
    })
}
