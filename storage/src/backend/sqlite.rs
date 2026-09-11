//! SQLite 后端
//!
//! **双池**是本后端的核心决策：
//!
//! - writer 池 `max_connections = 1`——把「单写者」从纪律变成物理约束。
//!   写事务在池内排队，写锁竞争从运行时错误变成可预期的等待。
//! - reader 池只读打开，误写会直接被 SQLite 拒绝（`SQLITE_READONLY`），
//!   读路径写库这类 bug 在第一次运行就暴露。
//!
//! 两池都显式设 `acquire_timeout`：sqlx 默认 30 秒，对单连接的写池意味着
//! 一个卡住的事务能把后续十几轮请求各自拖满 30 秒——排队本身成了故障放大器。
//!
//! 仓储实现进场时的纪律：读走 `reader`、写走 `writer`，每个方法单行委托到
//! 与方言无关的绑定层；选哪个池是本后端唯一保留的业务决策。

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use {{crate_prefix_snake}}_core::config::{SqliteConfig, StorageConfig};

use super::MAX_MIGRATION_VERSION;
use crate::error::{StorageError, StorageResult};
use crate::model::StorageHealth;
use crate::repo::Storage;

/// WAL 文件大小上限：64 MB。不设上限时 WAL 可能在长事务后无限膨胀。
const JOURNAL_SIZE_LIMIT: &str = "67108864";

/// 等一条空闲连接的上限。
///
/// 取 5 秒：比一轮业务操作的预算宽裕，又远短于人的忍耐。拿不到连接就快速失败，
/// 让调用方按自己的策略处理，而不是把线索埋在一次长等待里。
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SqliteStorage {
    writer: SqlitePool,
    reader: SqlitePool,
    /// 关停时重新收紧权限需要它（此时可能正好有新生成的边车文件）
    db_path: PathBuf,
}

/// 手写而非 derive：连接池的 `Debug` 会带出连接选项。只暴露库文件路径——
/// 它本就出现在启动日志里，且是排障时唯一真正有用的信息。
impl std::fmt::Debug for SqliteStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStorage")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

impl SqliteStorage {
    /// 连接 → 迁移 → 就绪。
    ///
    /// 路径由配置管线保证非空（留空时已推导为 `<安装根>/data/` 下的缺省文件）；
    /// 父目录不存在时创建（unix 下 0700）。
    pub(crate) async fn connect(cfg: &StorageConfig) -> StorageResult<Self> {
        let db_path = resolve_db_path(&cfg.sqlite)?;
        ensure_parent_dir(&db_path)?;

        let busy_timeout = Duration::from_millis(cfg.sqlite.busy_timeout_ms);
        let writer_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            // NORMAL：WAL 模式下仍能在进程崩溃时保持一致，只在整机断电时可能
            // 丢失最后几个事务。相对 FULL 的每事务 fsync，写吞吐差一个数量级。
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(busy_timeout)
            .foreign_keys(true)
            .pragma("journal_size_limit", JOURNAL_SIZE_LIMIT)
            // 新库在建表前生效；既有的非 INCREMENTAL 库需 VACUUM 才能切换
            .pragma("auto_vacuum", "INCREMENTAL");

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(writer_opts)
            .await?;

        // 此后任一步失败都必须先关掉已建好的 writer 池再上抛：
        // 迁移校验和不符是常见启动失败，而调用方多半会重试启动，
        // 用 `?` 直接返回会让每次重试都留下一个仍持有文件句柄的池
        if let Err(error) = sqlx::migrate!("migrations/sqlite").run(&writer).await {
            writer.close().await;
            return Err(error.into());
        }

        // 只读连接不能携带 journal_mode / auto_vacuum 等写型 PRAGMA
        // （会触发 SQLITE_READONLY），因此单独构造最小选项集
        let reader_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .read_only(true)
            .busy_timeout(busy_timeout)
            .foreign_keys(true);
        let readers = cfg.sqlite.read_pool_size.max(1);
        let reader = match SqlitePoolOptions::new()
            .max_connections(readers)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(reader_opts)
            .await
        {
            Ok(pool) => pool,
            Err(error) => {
                writer.close().await;
                return Err(error.into());
            }
        };

        restrict_permissions(&db_path);

        tracing::info!(
            path = %db_path.display(),
            readers,
            "SQLite storage connected (writer=1)"
        );

        Ok(Self {
            writer,
            reader,
            db_path,
        })
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    fn backend_name(&self) -> &'static str {
        "sqlite"
    }

    async fn health(&self) -> StorageResult<StorageHealth> {
        // 连通性探测走只读池：读路径饱和才是对外可见的故障
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.reader)
            .await?;

        let db_size_bytes: i64 = sqlx::query_scalar(
            "SELECT pc.page_count * ps.page_size FROM pragma_page_count() pc, pragma_page_size() ps",
        )
        .fetch_one(&self.reader)
        .await?;

        let migration_version: Option<i64> = sqlx::query_scalar(MAX_MIGRATION_VERSION)
            .fetch_one(&self.reader)
            .await?;

        Ok(StorageHealth {
            backend: self.backend_name(),
            healthy: true,
            db_size_bytes,
            migration_version,
        })
    }

    async fn run_maintenance(&self) -> StorageResult<()> {
        // checkpoint 会返回一行统计，用 fetch_all 吞掉而不是 execute
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_all(&self.writer)
            .await?;
        sqlx::query("PRAGMA incremental_vacuum")
            .fetch_all(&self.writer)
            .await?;
        sqlx::query("ANALYZE").execute(&self.writer).await?;
        tracing::debug!("SQLite maintenance completed");
        Ok(())
    }

    async fn close(&self) {
        // 顺序不能反：SQLite 仅在**最后一个**连接断开时做收尾 checkpoint 并
        // 删除 -wal/-shm，而只读连接无权做这件事。先关写池会把只读连接留到
        // 最后，结果是 WAL 常驻磁盘，下次启动多一轮重放。
        self.reader.close().await;
        self.writer.close().await;
        restrict_permissions(&self.db_path);
    }
}

// ── 辅助 ─────────────────────────────────────────────────────────────────────

fn resolve_db_path(cfg: &SqliteConfig) -> StorageResult<PathBuf> {
    if cfg.path.is_empty() {
        // 配置管线（`AppConfig::resolve_paths`）会把空路径推导成缺省文件；
        // 走到这里说明调用方绕过了管线直接构造配置，这是编程错误而不是部署错误
        return Err(StorageError::Init(
            "[storage.sqlite].path is empty; construct the config through the loading pipeline"
                .into(),
        ));
    }
    Ok(PathBuf::from(&cfg.path))
}

fn ensure_parent_dir(db_path: &Path) -> StorageResult<()> {
    let Some(parent) = db_path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).map_err(|e| {
        StorageError::Init(format!(
            "failed to create data directory {}: {e}",
            parent.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// 数据库文件及其 WAL / SHM 边车文件权限收紧为 0600。
///
/// 边车文件不能漏：-wal 里是尚未 checkpoint 的真实数据行，SQLite 创建它们时
/// 取 umask 默认（通常 0644）。只锁主库文件等于把门锁了而窗开着。
///
/// 失败仅告警不阻断：某些文件系统（如挂载的 exFAT）不支持 unix 权限位，
/// 为此拒绝启动是过度反应——数据目录本身已是 0700。
fn restrict_permissions(db_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let targets = [
            db_path.to_path_buf(),
            append_suffix(db_path, "-wal"),
            append_suffix(db_path, "-shm"),
        ];
        for path in &targets {
            if path.exists()
                && std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_err()
            {
                tracing::warn!(
                    path = %path.display(),
                    "failed to restrict database file permissions to 0600"
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = db_path;
    }
}

/// 追加文件名后缀（不是扩展名）：`a.db` → `a.db-wal`。
/// `set_extension` 会把 `.db` 替掉，在这里是错的。
#[cfg(unix)]
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_path_is_a_programming_error() {
        let cfg = SqliteConfig::default();
        assert!(matches!(resolve_db_path(&cfg), Err(StorageError::Init(_))));
    }

    #[test]
    fn ensure_parent_dir_creates_missing_directories() {
        let dir = std::env::temp_dir().join(format!(
            "{{crate_name}}-sqlite-parent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = dir.join("nested").join("x.db");
        ensure_parent_dir(&db).unwrap();
        assert!(db.parent().unwrap().is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn append_suffix_keeps_the_extension() {
        assert_eq!(
            append_suffix(Path::new("/x/a.db"), "-wal"),
            PathBuf::from("/x/a.db-wal")
        );
    }
}
