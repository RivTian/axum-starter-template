//! Temporary test-only schemas exercise the same connect/migrate/probe pipeline.
//! They are never embedded in the service's production migration directory.
use super::*;
use sqlx::migrate::Migrator;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::timeout;

const BUDGET: Duration = Duration::from_secs(5);
const PROBE: &str =
    "CREATE TABLE fixture_probe (value INTEGER); INSERT INTO fixture_probe VALUES (7);";

fn settings(path: PathBuf) -> StorageConfig {
    StorageConfig::new(
        path,
        1,
        0,
        Duration::from_secs(1),
        Duration::from_millis(250),
    )
    .unwrap()
}

async fn fixture(sql: &str) -> Migrator {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("0001_fixture.sql"), sql).unwrap();
    Migrator::new(dir.path()).await.unwrap()
}

async fn empty_fixture() -> Migrator {
    let dir = tempfile::tempdir().unwrap();
    Migrator::new(dir.path()).await.unwrap()
}

async fn rows(owner: &StorageOwner) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM fixture_probe")
        .fetch_one(&owner.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_real_migration_is_applied_once_and_validated_on_restart() {
    timeout(BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let config = settings(dir.path().join("service.sqlite3"));
        let migrations = fixture(PROBE).await;
        for _ in 0..2 {
            let owner = StorageOwner::prepare(config.clone());
            owner
                .initialize_with(&migrations)
                .await
                .unwrap()
                .health()
                .await
                .unwrap();
            assert_eq!(rows(&owner).await, 1);
            let versions: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = TRUE")
                    .fetch_one(&owner.pool)
                    .await
                    .unwrap();
            assert_eq!(versions, 1);
            owner.close().await;
        }
    })
    .await
    .expect("migration restart probe exceeded deadline");
}

#[tokio::test]
async fn checksum_missing_version_dirty_and_sql_errors_fail_without_repair() {
    timeout(BUDGET, async {
        for case in ["checksum", "missing", "dirty", "sql"] {
            let dir = tempfile::tempdir().unwrap();
            let config = settings(dir.path().join("service.sqlite3"));
            let owner = StorageOwner::prepare(config.clone());
            let original = fixture(PROBE).await;
            if case != "sql" {
                owner.initialize_with(&original).await.unwrap();
            }
            if case == "dirty" {
                sqlx::query("UPDATE _sqlx_migrations SET success = FALSE")
                    .execute(&owner.pool)
                    .await
                    .unwrap();
            }
            let empty = tempfile::tempdir().unwrap();
            let candidate = match case {
                "checksum" => fixture(&format!("{PROBE}\n-- changed checksum")).await,
                "missing" => Migrator::new(empty.path()).await.unwrap(),
                "sql" => fixture("CREATE TABLE fixture_probe (value INTEGER); INVALID_SQL;").await,
                _ => original,
            };
            let error = owner
                .initialize_with(&candidate)
                .await
                .err()
                .expect("bad migration was accepted");
            assert_eq!(error.phase(), "migrate", "{case}");
            assert!(!error.to_string().contains("INVALID_SQL"));
            if case != "sql" {
                assert_eq!(rows(&owner).await, 1);
                let checksum: Vec<u8> =
                    sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 1")
                        .fetch_one(&owner.pool)
                        .await
                        .unwrap();
                assert_eq!(
                    checksum.as_slice(),
                    fixture(PROBE)
                        .await
                        .iter()
                        .next()
                        .unwrap()
                        .checksum
                        .as_ref()
                );
                let success: bool =
                    sqlx::query_scalar("SELECT success FROM _sqlx_migrations WHERE version = 1")
                        .fetch_one(&owner.pool)
                        .await
                        .unwrap();
                assert_eq!(success, case != "dirty");
            } else {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
                    .fetch_one(&owner.pool)
                    .await
                    .unwrap();
                assert_eq!(count, 0, "failed SQL was recorded as applied");
            }
            owner.close().await;
            assert!(owner.pool.is_closed());
        }
    })
    .await
    .expect("migration rejection probes exceeded deadline");
}

#[tokio::test]
async fn two_initializers_compete_boundedly_and_restart_revalidates_the_winner() {
    timeout(BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let config = settings(dir.path().join("shared.sqlite3"));
        let migrations = fixture(PROBE).await;
        let left = StorageOwner::prepare(config.clone());
        let right = StorageOwner::prepare(config.clone());
        let (a, b) = tokio::join!(
            left.initialize_with(&migrations),
            right.initialize_with(&migrations)
        );
        // SQLite's migration advisory lock is a no-op. A contending initializer
        // may fail; no retry or claim that both writers must succeed is added.
        assert!(a.is_ok() || b.is_ok());
        for failure in [a.err(), b.err()].into_iter().flatten() {
            assert!(matches!(failure.phase(), "connect" | "migrate"));
        }
        left.close().await;
        right.close().await;
        let restarted = StorageOwner::prepare(config);
        restarted.initialize_with(&migrations).await.unwrap();
        assert_eq!(rows(&restarted).await, 1);
        restarted.close().await;
    })
    .await
    .expect("contending migrations exceeded deadline");
}

#[tokio::test]
async fn an_interrupted_transactional_migration_is_revalidated_on_restart() {
    timeout(BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let config = settings(dir.path().join("interrupted.sqlite3"));
        let owner = StorageOwner::prepare(config.clone());
        owner.initialize_with(&empty_fixture().await).await.unwrap();
        let interrupted = Arc::new(AtomicBool::new(false));
        {
            let mut connection = owner.pool.acquire().await.unwrap();
            let seen = interrupted.clone();
            connection.lock_handle().await.unwrap().set_progress_handler(10_000, move || {
                seen.store(true, Ordering::SeqCst);
                false // Safe driver API: interrupt SQL, never block a worker thread.
            });
        }
        let migrations = fixture("CREATE TABLE fixture_probe (value INTEGER); WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v < 10000) INSERT INTO fixture_probe SELECT v FROM n;").await;
        let error = owner.initialize_with(&migrations).await.err().expect("SQL was not interrupted");
        assert_eq!(error.phase(), "migrate");
        assert!(interrupted.load(Ordering::SeqCst));
        owner.close().await;
        let restarted = StorageOwner::prepare(config);
        restarted.initialize_with(&migrations).await.unwrap();
        assert_eq!(rows(&restarted).await, 10_000);
        restarted.close().await;
        // This evidence is for this transactional fixture, not a promise about
        // power loss or user-written nontransactional migrations.
    }).await.expect("interrupted migration recovery exceeded deadline");
}

#[tokio::test]
async fn an_exclusive_database_lock_times_out_without_publishing_storage() {
    timeout(BUDGET, async {
        let dir = tempfile::tempdir().unwrap();
        let config = settings(dir.path().join("locked.sqlite3"));
        let holder = StorageOwner::prepare(config.clone());
        holder
            .initialize_with(&empty_fixture().await)
            .await
            .unwrap();
        let mut connection = holder.pool.acquire().await.unwrap();
        sqlx::query("PRAGMA journal_mode=DELETE")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("BEGIN EXCLUSIVE")
            .execute(&mut *connection)
            .await
            .unwrap();
        let contender = StorageOwner::prepare(config.clone());
        let migrations = fixture(PROBE).await;
        let error = contender
            .initialize_with(&migrations)
            .await
            .err()
            .expect("exclusive lock was ignored");
        assert!(matches!(error.phase(), "connect" | "migrate"));
        contender.close().await;
        assert!(contender.pool.is_closed());
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        holder.close().await;
        let restarted = StorageOwner::prepare(config);
        let facade = restarted.initialize_with(&migrations).await.unwrap();
        assert_eq!(rows(&restarted).await, 1);
        restarted.close().await;
        assert_eq!(facade.health().await.unwrap_err().phase(), "health");
    })
    .await
    .expect("exclusive lock probe exceeded deadline");
}
