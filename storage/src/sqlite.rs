//! 池的构造、PRAGMA、以及 `Storage` 的 SQLite 实现。
//!
//! # 双池
//!
//! writer 的 `max_connections = 1`。这不是调优，是把 SQLite 的**物理性质**写进类型里：
//! 一个库同一时刻只有一个写者。池子开大只会把"写冲突"从池外挪到池内，变成一堆
//! `SQLITE_BUSY`，而那时错误发生的位置离原因更远了。
//!
//! reader 是 `read_only`。它的作用不是性能，是**兜底**：拿错池去写会当场拿到
//! `SQLITE_READONLY`（归一化成 `Internal`，见 `error_map`），而不是绕过单写者纪律成功写进去。
//!
//! # 每条 PRAGMA 都显式写
//!
//! 「默认值恰好合适」和「我们选择了这个值」在代码里长得一样，但在别人升级依赖那天不一样。
//! 下面每一条都写明了默认是什么、为什么不接受默认。

use std::fmt;

use service_core::config::StorageConfig;
use service_core::storage::{Storage, StorageError, StorageFuture};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error_map;

/// 后端名。出现在 `StorageError::Unavailable { backend }` 和 `Debug for dyn Storage` 里。
pub(crate) const BACKEND: &str = "sqlite";

/// 两个池共用的部分。
fn base_options(cfg: &StorageConfig) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(&cfg.path)
        // **不让驱动建文件。** 默认就是 `false`，这里显式写出来是因为理由不显然：库文件由
        // `open` 自己先建好并设成 0600（见 `open::prepare_file`）。交给驱动建的话，文件模式
        // 由 umask 决定，多数环境下是 0644——一个"权限收紧"的纪律会因为一个默认值而失效，
        // 且没有任何报错。留 `false` 还有一个好处：文件不存在时报 `SQLITE_CANTOPEN`，
        // 说明我们自己那一步出了问题，比一个权限不对的新库好查。
        .create_if_missing(false)
        // 默认是**关的**。SQLite 出于向后兼容一直没打开外键，于是"建了外键但它不生效"是
        // 一个极其常见的线上意外。
        .foreign_keys(true)
        // 拿不到锁时在驱动内部重试多久。默认没有这一项（拿不到就立刻 `SQLITE_BUSY`）。
        // 它与池的 `acquire_timeout` 是两件事：那个是"池里没有空闲连接"，这个是"连接拿到了
        // 但库被别人锁着"。
        .busy_timeout(cfg.busy_timeout)
}

/// 写池的连接参数。
fn writer_options(cfg: &StorageConfig) -> SqliteConnectOptions {
    base_options(cfg)
        // 默认是 `DELETE`（回滚日志）。选 WAL 是因为它让读不阻塞写、写不阻塞读——双池的
        // 前提。代价是多出 `-wal` / `-shm` 两个边车文件，备份时必须连它们一起（否则恢复出来
        // 的库会缺最近的事务）。
        .journal_mode(SqliteJournalMode::Wal)
        // WAL 模式下 sqlx 的默认值是 `NORMAL`：掉电可能丢掉最近几个事务。这是一个**服务
        // 模板**，不知道使用者会往里放什么，所以取 `FULL`——每次提交都 fsync。慢，但"慢"
        // 是能测出来的，"偶尔丢一个事务"不是。需要吞吐的人可以改这一行，那时他知道自己在
        // 放弃什么。
        .synchronous(SqliteSynchronous::Full)
}

/// 读池的连接参数。
fn reader_options(cfg: &StorageConfig) -> SqliteConnectOptions {
    base_options(cfg).read_only(true)
    // 这里**没有** `journal_mode`。`PRAGMA journal_mode = WAL` 是一次写操作，只读连接执行
    // 不了；写池已经把库设成 WAL 了，只读连接连上去读到的就是 WAL（实测确认）。
}

/// 建写池。
///
/// `connect_with` 会**立刻**取一条连接，所以连接参数有问题时这里当场就报真正的错误，
/// 而不是等到第一次查询。
pub(crate) async fn writer_pool(cfg: &StorageConfig) -> Result<SqlitePool, StorageError> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(cfg.acquire_timeout)
        .connect_with(writer_options(cfg))
        .await
        .map_err(error_map::from_sqlx)
}

/// 建读池。
///
/// `cfg.max_readers` 为 0 时不在这里兜底：`core` 的配置流水线已经把它钳到 1 了
/// （`core/src/config/pipeline.rs` 的钳位表），在这里再写一遍就有了两个实现，而两个实现
/// 迟早分叉。真有人绕过流水线直接构造配置，结果是 `acquire_timeout` 之后报
/// `Unavailable`——一个有界的失败，不是挂起。
pub(crate) async fn reader_pool(cfg: &StorageConfig) -> Result<SqlitePool, StorageError> {
    SqlitePoolOptions::new()
        .max_connections(cfg.max_readers)
        .acquire_timeout(cfg.acquire_timeout)
        .connect_with(reader_options(cfg))
        .await
        .map_err(error_map::from_sqlx)
}

/// 交给上层的门面实现。
///
/// 它**只持有读池**。理由见 [`Storage::health`] 的实现注释。
pub(crate) struct SqliteStorage {
    reader: SqlitePool,
}

impl SqliteStorage {
    pub(crate) fn new(reader: SqlitePool) -> Self {
        Self { reader }
    }
}

impl fmt::Debug for SqliteStorage {
    /// 只输出后端名。
    ///
    /// 手写而不是 `derive`：`derive` 会把 `SqlitePool` 的 `Debug` 展开，里面有库文件路径和
    /// 池的内部状态。`Arc<dyn Storage>` 会被 `app` 放进结构体，而结构体迟早会被 `{:?}` 进
    /// 日志——那时路径就泄漏了。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(BACKEND)
    }
}

impl Storage for SqliteStorage {
    /// 探**读池**。
    ///
    /// 不探写池是有意的：写池只有一条连接，健康检查去占它，就等于让每一次探活和每一次真正
    /// 的写互相排队。探活的频率由外部决定（k8s 的 readiness 可以是每秒一次），让它能拖慢
    /// 写入是一个很容易触发、又很难联想到的故障。
    ///
    /// 读池连不上时写池多半也连不上；真出现"读坏写好"的情况，服务对外也确实不该说自己 ready。
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            sqlx::query("SELECT 1")
                .fetch_one(&self.reader)
                .await
                .map(|_row| ())
                .map_err(error_map::from_sqlx)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use service_testkit::TempDb;
    use sqlx::Row as _;

    use super::*;

    /// 指向一条临时库路径的配置。文件还不存在——建它是 `open` 的事，所以这里的用例都先
    /// 自己把写池建出来（写池的 `create_if_missing` 是 `false`，见上面的理由）。
    fn config_at(db: &TempDb) -> StorageConfig {
        StorageConfig {
            path: db.path().to_path_buf(),
            max_readers: 2,
            busy_timeout: Duration::from_millis(200),
            acquire_timeout: Duration::from_secs(2),
        }
    }

    /// 先把库文件摸出来，再建写池。用例里重复三次，抽出来。
    async fn writer_on(cfg: &StorageConfig) -> SqlitePool {
        std::fs::write(&cfg.path, b"").expect("摸出库文件");
        writer_pool(cfg).await.expect("建写池")
    }

    #[tokio::test]
    async fn writer_pool_is_physically_single_writer() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let writer = writer_on(&cfg).await;

        // 直接问池子自己，而不是数连接：断言的是**配置**，那才是这条纪律的载体。
        assert_eq!(
            writer.options().get_max_connections(),
            1,
            "写池必须是单连接：SQLite 的单写者是物理性质，不是可调参数"
        );
        writer.close().await;
    }

    #[tokio::test]
    async fn pragmas_are_what_we_asked_for_not_what_sqlite_defaults_to() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let writer = writer_on(&cfg).await;

        // 三条都是"默认值不是我们要的"。断言它们生效，等于断言 `base_options` /
        // `writer_options` 里那几行确实被执行了——删掉其中一行，这里就红。
        let cases: &[(&str, &str, &str)] = &[
            ("journal_mode", "PRAGMA journal_mode", "wal"),
            ("synchronous", "PRAGMA synchronous", "2"),
            ("foreign_keys", "PRAGMA foreign_keys", "1"),
        ];
        for (i, (name, sql, want)) in cases.iter().enumerate() {
            let row = sqlx::query(*sql)
                .fetch_one(&writer)
                .await
                .expect("读 PRAGMA");
            // PRAGMA 的返回列类型不统一（`journal_mode` 是文本，另外两个是整数），
            // 统一按文本取会失败，所以两种都试一下。
            let got = row
                .try_get::<String, _>(0)
                .unwrap_or_else(|_| row.get::<i64, _>(0).to_string())
                .to_lowercase();
            assert_eq!(&got, want, "TC{i} ({name}) 不是我们设的值");
        }
        writer.close().await;
    }

    #[tokio::test]
    async fn reader_pool_really_refuses_writes() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let writer = writer_on(&cfg).await;
        let reader = reader_pool(&cfg).await.expect("建读池");

        // 读池能读。
        sqlx::query("SELECT 1")
            .fetch_one(&reader)
            .await
            .expect("读池能读");

        // 读池不能写——而且归一化之后是 `internal`，不是 `unavailable`：拿错池是代码问题，
        // 报 503 会把人引到磁盘和负载上去查。
        let err = sqlx::query("CREATE TABLE t (c INTEGER)")
            .execute(&reader)
            .await
            .expect_err("只读池不该写得进去");
        let mapped = error_map::from_sqlx(err);
        assert_eq!(mapped.kind_str(), "internal", "实际：{mapped}");

        reader.close().await;
        writer.close().await;
    }

    #[tokio::test]
    async fn debug_of_the_facade_leaks_nothing_but_the_backend_name() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let writer = writer_on(&cfg).await;
        let reader = reader_pool(&cfg).await.expect("建读池");

        let facade = SqliteStorage::new(reader.clone());
        let rendered = format!("{facade:?}");
        assert_eq!(rendered, BACKEND);
        assert!(
            !rendered.contains(&db.path().display().to_string()),
            "库路径不该出现在 Debug 里：{rendered}"
        );

        // 顺带确认门面本身能用。
        facade.health().await.expect("刚建好的库应当是健康的");

        reader.close().await;
        writer.close().await;
    }

    #[tokio::test]
    async fn health_fails_once_the_pool_is_closed() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let writer = writer_on(&cfg).await;
        let reader = reader_pool(&cfg).await.expect("建读池");
        let facade = SqliteStorage::new(reader.clone());

        reader.close().await;
        let err = facade.health().await.expect_err("池关了之后探活必须失败");
        assert_eq!(
            err.kind_str(),
            "unavailable",
            "停机窗口里的探活应当报 unavailable（503），不是 internal（500）"
        );

        writer.close().await;
    }
}
