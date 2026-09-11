//! PostgreSQL 后端启动契约
//!
//! 需要一个真实 PostgreSQL 实例，通过环境变量提供连接参数；未提供时整组跳过
//! （打印一行提示而不是 panic：CI 与本地开发多数情况下没有 PG）。
//!
//! ```sh
//! {{env_prefix}}_TEST_PG_HOST=127.0.0.1 \
//! {{env_prefix}}_TEST_PG_DBNAME={{crate_name}}_test \
//! {{env_prefix}}_TEST_PG_USER=postgres \
//! {{env_prefix}}_TEST_PG_PASSWORD=postgres \
//! cargo test -p {{crate_prefix}}-storage --test postgres_bootstrap
//! ```
//!
//! 声明了 HOST 却连不上时这组测试**红而不跳**：静默跳过会让「PG 侧没跑过」
//! 看起来和「PG 侧全绿」一模一样。不用 SQLite 的假 PG 替身：方言差异恰恰是
//! 这组测试要覆盖的东西，用替身跑绿等于什么都没验证。

use {{crate_prefix_snake}}_core::config::{PostgresConfig, StorageBackend, StorageConfig};
use {{crate_prefix_snake}}_storage::init_storage;

mod support;

#[tokio::test]
async fn init_connects_and_reports_health() {
    let Some(cfg) = support::pg_config() else {
        let prefix = support::PG_ENV_PREFIX;
        eprintln!("skipped: {prefix}_HOST 未设置，无可用 PostgreSQL 实例");
        return;
    };

    let storage = init_storage(&cfg).await.expect("初始化必须成功");
    assert_eq!(storage.backend_name(), "postgres");

    let health = storage.health().await.unwrap();
    assert!(health.healthy);
    assert_eq!(health.backend, "postgres");
    assert!(health.db_size_bytes > 0);
    // 模板零迁移 ⇒ None；加第一个迁移时改成 Some(1)（同 SQLite 侧）
    assert_eq!(health.migration_version, None);

    storage.run_maintenance().await.expect("维护命令必须可执行");
    storage.close().await;
}

/// 连不通时必须 fail-fast 返回 Err，而不是返回一个「看起来能用」的门面。
/// 不需要真实 PG：保留端口 1 上不会有任何监听
#[tokio::test]
async fn init_fails_fast_on_unreachable_server() {
    let cfg = StorageConfig {
        backend: StorageBackend::Postgres,
        postgres: PostgresConfig {
            host: "127.0.0.1".into(),
            port: 1,
            sslmode: "disable".into(),
            connect_timeout_ms: 1_000,
            password_env: String::new(),
            ..PostgresConfig::default()
        },
        ..StorageConfig::default()
    };

    assert!(
        init_storage(&cfg).await.is_err(),
        "连不通数据库时不得返回可用门面"
    );
}
