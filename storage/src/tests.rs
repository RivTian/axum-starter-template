use super::*;
use tokio::time::timeout;

fn config(path: std::path::PathBuf) -> StorageConfig {
    StorageConfig::new(path, 1, 0, Duration::from_secs(2), Duration::from_secs(1)).unwrap()
}

const TEST_BUDGET: Duration = Duration::from_secs(5);

#[tokio::test]
async fn empty_migrations_bootstrap_only_sqlx_metadata_and_can_restart() {
    timeout(TEST_BUDGET, async {
        assert_eq!(MIGRATOR.iter().count(), 0, "no business migrations in M1");
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path().join("service.sqlite3"));
        for _ in 0..2 {
            let owner = StorageOwner::prepare(config.clone());
            let storage = owner.initialize().await.unwrap();
            storage.health().await.unwrap();
            let tables: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            ).fetch_all(&owner.pool).await.unwrap();
            assert_eq!(tables, ["_sqlx_migrations"]);
            let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
                .fetch_one(&owner.pool).await.unwrap();
            assert_eq!(applied, 0);
            let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&owner.pool).await.unwrap();
            assert_eq!(foreign_keys, 1);
            owner.close().await;
            assert_eq!(storage.health().await.unwrap_err().phase(), "health");
        }
    }).await.expect("empty migration probe exceeded its deadline");
}

#[tokio::test]
async fn a_lazy_pool_is_not_a_successful_startup() {
    timeout(TEST_BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let owner = StorageOwner::prepare(config(
            dir.path().join("missing-parent").join("service.sqlite3"),
        ));
        let error = match owner.initialize().await {
            Ok(_) => panic!("invalid storage must not publish a facade"),
            Err(error) => error,
        };
        assert_eq!(error.phase(), "connect");
        assert!(error.source().is_some());
        owner.close().await;
    })
    .await
    .expect("failed initialization did not remain bounded");
}

#[tokio::test]
async fn cancelling_initialization_keeps_the_pool_owner_available_for_close() {
    timeout(TEST_BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let owner = StorageOwner::prepare(config(dir.path().join("cancel.sqlite3")));
        // Exhaust the one-connection pool so the initialization future genuinely waits.
        let held = owner.pool.acquire().await.unwrap();
        assert!(
            timeout(Duration::from_millis(20), owner.initialize())
                .await
                .is_err()
        );
        drop(held);
        owner.close().await;
        assert!(owner.pool.is_closed());
    })
    .await
    .expect("cancelled initialization leaked its close owner");
}

#[tokio::test]
async fn options_apply_to_every_connection_and_close_waits_for_a_borrower() {
    timeout(TEST_BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let cfg = StorageConfig::new(
            dir.path().join("connections.sqlite3"),
            2,
            0,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
        let owner = StorageOwner::prepare(cfg);
        let _facade = owner.initialize().await.unwrap();
        let mut a = owner.pool.acquire().await.unwrap();
        let mut b = owner.pool.acquire().await.unwrap();
        for connection in [&mut a, &mut b] {
            let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            assert_eq!(enabled, 1);
        }
        drop(b);
        assert!(
            timeout(Duration::from_millis(20), owner.close())
                .await
                .is_err()
        );
        drop(a);
        owner.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unknown_or_failed_migration_records_prevent_publishing_storage() {
    timeout(TEST_BUDGET, async {
        for success in [true, false] {
            let dir = tempfile::tempdir().unwrap(); let cfg = config(dir.path().join("migration.sqlite3"));
            let owner = StorageOwner::prepare(cfg.clone()); let facade = owner.initialize().await.unwrap();
            sqlx::query("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (1, 'test-only record', ?, ?, 0)")
                .bind(success).bind(vec![0_u8; 48]).execute(&owner.pool).await.unwrap();
            drop(facade); owner.close().await;
            let next = StorageOwner::prepare(cfg);
            let error = match next.initialize().await { Ok(_) => panic!("bad migration metadata was accepted"), Err(error) => error };
            assert_eq!(error.phase(), "migrate"); next.close().await;
        }
    }).await.unwrap();
}
