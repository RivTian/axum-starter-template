//! `Db` 门面的行为测试：开池、迁移记账、busy_timeout、关池、只支持 sqlite。

use std::time::Duration;

use {{crate_prefix_snake}}_storage::{Db, ErrorKind};
use tempfile::TempDir;

fn url(temp: &TempDir) -> String {
    format!("sqlite:{}/test.db?mode=rwc", temp.path().display())
}

#[tokio::test]
async fn pool_opens_and_migrations_apply() {
    let temp = TempDir::new().expect("临时目录");
    let db = Db::open(&url(&temp), Duration::from_millis(500))
        .await
        .expect("开池");

    db.migrate().await.expect("空迁移集也必须能跑");

    // 迁移运行器建立了记账表：这是"迁移真的跑过"的证据，而不是"没报错"。
    let recorded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(db.pool())
        .await
        .expect("迁移记账表必须存在");
    assert_eq!(recorded, 0, "骨架里不该有迁移");

    // 仓储实现要用的查询路径是通的。
    sqlx::query("CREATE TABLE probe (id INTEGER PRIMARY KEY, note TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .expect("建表");
    sqlx::query("INSERT INTO probe (note) VALUES (?)")
        .bind("hello")
        .execute(db.pool())
        .await
        .expect("写入");
    let note: String = sqlx::query_scalar("SELECT note FROM probe WHERE id = 1")
        .fetch_one(db.pool())
        .await
        .expect("读取");
    assert_eq!(note, "hello");

    db.close().await.expect("关池");
}

#[tokio::test]
async fn busy_timeout_is_applied_to_connections() {
    let temp = TempDir::new().expect("临时目录");
    let db = Db::open(&url(&temp), Duration::from_millis(250))
        .await
        .expect("开池");

    let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(db.pool())
        .await
        .expect("读 pragma");
    assert_eq!(timeout, 250);

    db.close().await.expect("关池");
}

#[tokio::test]
async fn closing_is_idempotent_from_the_outside_and_marks_pool_closed() {
    let temp = TempDir::new().expect("临时目录");
    let db = Db::open(&url(&temp), Duration::from_millis(500))
        .await
        .expect("开池");
    let pool = db.pool().clone();
    db.close().await.expect("关池");
    assert!(pool.is_closed(), "关池之后不该还能拿连接");
}

#[tokio::test]
async fn unsupported_scheme_is_rejected_with_a_clear_message() {
    let err = Db::open(
        "postgres://user:secret@localhost/db",
        Duration::from_millis(500),
    )
    .await
    .expect_err("骨架只支持 sqlite");
    assert_eq!(err.kind(), ErrorKind::Startup);
    assert!(err.to_string().contains("sqlite:"), "{err}");
    assert!(
        !err.to_string().contains("secret"),
        "错误消息里不能带连接串（可能含凭据）：{err}"
    );
}
