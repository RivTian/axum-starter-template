//! A single SQLite backend, a read-only dynamic facade and an app-owned closer.
//! No pools, connections, SQLx errors or migration handles in the public API.

use std::error::Error;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

// Immutable build metadata, not a process-global connection or business state.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Storage: Send + Sync {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>>;
}

/// Validated component parameters, independent of the file/configuration loader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageConfig {
    path: PathBuf,
    max_connections: u32,
    min_connections: u32,
    acquire_timeout: Duration,
    busy_timeout: Duration,
}

impl StorageConfig {
    pub fn new(
        path: PathBuf,
        max_connections: u32,
        min_connections: u32,
        acquire_timeout: Duration,
        busy_timeout: Duration,
    ) -> Result<Self, StorageConfigError> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(StorageConfigError(
                "storage.path must be an absolute file path",
            ));
        }
        if !(1..=64).contains(&max_connections) || min_connections > max_connections {
            return Err(StorageConfigError(
                "storage pool requires 0 <= min <= max <= 64 and max > 0",
            ));
        }
        if acquire_timeout.is_zero()
            || busy_timeout.is_zero()
            || acquire_timeout > Duration::from_secs(60)
            || busy_timeout > Duration::from_secs(60)
        {
            return Err(StorageConfigError("storage timeouts must be in (0, 60s]"));
        }
        Ok(Self {
            path,
            max_connections,
            min_connections,
            acquire_timeout,
            busy_timeout,
        })
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StorageConfigError(&'static str);

/// Keep this owner outside the initialization future, including on cancellation.
/// Only app receives it; HTTP receives the health-only `Arc<dyn Storage>`.
pub struct StorageOwner {
    pool: SqlitePool,
}

impl StorageOwner {
    /// Must be called inside the main runtime. This does not declare readiness:
    /// the lazy pool makes it possible to retain ownership before the first await.
    pub fn prepare(config: StorageConfig) -> Self {
        // Every pragma below must be requested explicitly. SqliteConnectOptions
        // emits a pragma only for the ones a caller sets; the `Default` impls on
        // SqliteJournalMode/SqliteSynchronous are not what an unset field means.
        //
        // journal_mode: without WAL a pooled writer holds an exclusive lock over
        // the whole database and blocks every reader, so a multi-connection pool
        // turns ordinary concurrency into SQLITE_BUSY. WAL is a persistent
        // database attribute and switching into it needs an exclusive lock that
        // busy_timeout cannot wait on, which is why it is set here, on the first
        // connection of a lazy pool, rather than after migrations. Converting a
        // pre-existing non-WAL file that another process still holds open fails
        // at the "connect" phase and aborts startup; it never degrades silently.
        //
        // synchronous: NORMAL under WAL never corrupts the database, and a
        // process crash loses nothing because committed data already sits in the
        // OS page cache. An OS crash or power loss can roll back transactions
        // committed since the last checkpoint. Raise this to FULL if losing them
        // is unacceptable for the data being stored.
        let options = SqliteConnectOptions::new()
            .filename(config.path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(config.busy_timeout);
        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.acquire_timeout)
            .connect_lazy_with(options);
        Self { pool }
    }

    /// Connect, migrate, probe, then publish. The caller supplies the startup
    /// deadline and retains this owner if that future is cancelled or fails.
    pub async fn initialize(&self) -> Result<Arc<dyn Storage>, StorageError> {
        self.initialize_with(&MIGRATOR).await
    }

    // Private seam for real migration fixtures; production always uses the one
    // embedded set above. No driver/migrator handle crosses the crate boundary.
    async fn initialize_with(
        &self,
        migrator: &sqlx::migrate::Migrator,
    ) -> Result<Arc<dyn Storage>, StorageError> {
        {
            let _connection = self
                .pool
                .acquire()
                .await
                .map_err(|error| StorageError::new("connect", error))?;
        }
        migrator
            .run(&self.pool)
            .await
            .map_err(|error| StorageError::new("migrate", error))?;
        let facade: Arc<dyn Storage> = Arc::new(SqliteStorage {
            pool: self.pool.clone(),
        });
        facade.health().await?;
        Ok(facade)
    }

    /// Poll only after users have drained, and under the caller's close deadline.
    /// An async function avoids starting pool closure when merely constructing it.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

struct SqliteStorage {
    pool: SqlitePool,
}

impl Storage for SqliteStorage {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            sqlx::query_scalar::<_, i64>("SELECT 1")
                .fetch_one(&self.pool)
                .await
                .map_err(|error| StorageError::new("health", error))?;
            Ok(())
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("storage {phase} failed")]
pub struct StorageError {
    phase: &'static str,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl StorageError {
    /// Allows alternate Storage implementations (including fakes) to report an
    /// unavailable dependency without constructing any SQLx type.
    pub fn unavailable() -> Self {
        Self::new("health", std::io::Error::other("storage unavailable"))
    }

    fn new(phase: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            phase,
            source: Box::new(source),
        }
    }

    pub fn phase(&self) -> &'static str {
        self.phase
    }
}

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod tests;
