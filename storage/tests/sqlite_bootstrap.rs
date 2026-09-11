//! SQLite 后端启动契约
//!
//! 这些断言钉住的是「框架层」承诺，与具体业务表无关，因此在后续每次加表时
//! 都应保持通过——尤其是「不预建业务表」那条，它是防止 schema 提前腐化的守卫。

use {{crate_prefix_snake}}_storage::init_storage;

mod support;

#[tokio::test]
async fn init_creates_database_and_reports_health() {
    let (db, cfg) = support::sqlite_config("sqlite_bootstrap", "health");
    let storage = init_storage(&cfg).await.expect("初始化必须成功");

    assert!(db.exists(), "缺文件时应自动建库");
    assert_eq!(storage.backend_name(), "sqlite");

    let health = storage.health().await.unwrap();
    assert!(health.healthy);
    assert_eq!(health.backend, "sqlite");
    assert!(health.db_size_bytes > 0, "已建库的文件不应为空");
    // 模板零迁移 ⇒ None。加第一个迁移时把这里改成 Some(1)：它钉的是
    // 「迁移确实跑到了最后一个」，写死数字才能在漏跑时立刻发现
    assert_eq!(health.migration_version, None);

    storage.run_maintenance().await.expect("维护命令必须可执行");
    storage.close().await;
}

/// 迁移集应建出**且仅建出**当前有消费者的表——模板没有消费者，所以只有
/// sqlx 自己的记录表。多出任何一张表都是预建：没有消费者的表无人维护。
#[tokio::test]
async fn migration_set_creates_no_business_tables() {
    let (db, cfg) = support::sqlite_config("sqlite_bootstrap", "notables");
    let storage = init_storage(&cfg).await.unwrap();
    storage.close().await;

    // 直连原库核对表清单：绕开门面，避免用被测代码验证被测代码
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", db.display()))
        .await
        .unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    pool.close().await;

    assert_eq!(tables, vec!["_sqlx_migrations".to_string()]);
}

/// 关闭后门面不可用：`health()` 报错是「池真的关了」的唯一外部证据
#[tokio::test]
async fn close_makes_the_facade_unusable() {
    let (_db, cfg) = support::sqlite_config("sqlite_bootstrap", "close");
    let storage = init_storage(&cfg).await.unwrap();
    storage.close().await;
    assert!(storage.health().await.is_err(), "关池后 health 应报错");
}

/// 库文件权限收紧为 0600：-wal 里有真实数据行，不能按 umask 默认落盘
#[cfg(unix)]
#[tokio::test]
async fn database_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let (db, cfg) = support::sqlite_config("sqlite_bootstrap", "perm");
    let storage = init_storage(&cfg).await.unwrap();
    storage.close().await;

    let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "库文件应为 0600，实际 {mode:o}");
}
