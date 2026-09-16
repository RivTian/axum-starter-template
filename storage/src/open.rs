//! 唯一入口：连接 → 迁移 → 自检。
//!
//! 这个模块是**打开存储的唯一途径**，也是关闭它的唯一途径。别处拿不到池——`StorageOwner`
//! 的字段是私有的，`Arc<dyn Storage>` 上只有 `health()`。这条纪律不是靠评审维持的：
//! `sqlx` 类型在 `lib.rs` 的公共出口里一个都没有。
//!
//! # 谁负责关
//!
//! `open` 返回**两样东西**：一个 owner 和一个门面。门面可以随便克隆、传给任何层；owner
//! 只有一份，`close` 按值消费它。于是"谁负责关"在类型上就是唯一的——`app` 持有 owner，
//! 在关停序列的倒数第二步调用 `close`。

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use service_core::config::StorageConfig;
use service_core::storage::{CloseOutcome, Storage, StorageError};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use tokio_util::sync::CancellationToken;

use crate::error_map;
use crate::sqlite::{self, SqliteStorage};

/// 编译期内嵌的迁移集。
///
/// **不导出。** 导出它等于把 `sqlx::migrate::Migrator` 放进公共 API，那是本层最努力避免的
/// 事。要验证迁移集的内容，写在这个 crate 的用例里。
///
/// 目录改动能触发重编，靠的是 `../build.rs`；那个文件不可删，理由见 `migrations/README.md`。
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// 存储的所有者。持有两个池，是唯一能关闭它们的东西。
///
/// 没有 `Clone`：所有权唯一是"谁负责关"这个问题只有一个答案的原因。
pub struct StorageOwner {
    /// 单连接写池。
    writer: SqlitePool,
    /// 只读池，也是门面探活走的那个。
    reader: SqlitePool,
}

impl std::fmt::Debug for StorageOwner {
    /// 与 `Debug for dyn Storage` 同一条纪律：只输出后端名，不输出路径和池状态。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageOwner")
            .field("backend", &sqlite::BACKEND)
            .finish()
    }
}

impl StorageOwner {
    /// 打开存储：建库文件 → 写池 → 迁移 → 读池 → 自检。
    ///
    /// 任一步失败都 fail-fast，并且**在上抛之前先把已经建好的池关掉**。留一个活着的池再
    /// 上抛，会让"启动失败"变成"启动失败但库还被占着"，下一次重试撞到的是锁而不是原始原因。
    ///
    /// 全程受 `cancel` 约束。启动期间收到停止信号时，每一步之间都会让出并优先响应取消
    /// （`select!` 的 `biased`）——一个还在迁移的进程不应该让 Ctrl-C 等到迁移跑完。
    ///
    /// # Errors
    ///
    /// - 建目录 / 建库文件失败 → [`StorageError::Internal`]
    /// - 连不上 → [`StorageError::Unavailable`]
    /// - 迁移的三类元数据异常 → [`StorageError::Migration`]（**不自动修复**）
    /// - 被取消 → [`StorageError::Internal`]，`context` 里带 `cancelled`
    pub async fn open(
        cfg: &StorageConfig,
        cancel: &CancellationToken,
    ) -> Result<(Self, Arc<dyn Storage>), StorageError> {
        // 文件先由我们自己建，权限才受控——交给驱动建的话模式由 umask 决定。
        // 顺带一提，这一步也是 `-wal` / `-shm` 能拿到 0600 的原因：边车文件在创建时**继承
        // 主库文件的模式**（实测），所以主库先是 0600，边车就不用单独 chmod。
        prepare_file(&cfg.path)?;

        let writer = under_cancel(cancel, "connect", sqlite::writer_pool(cfg)).await?;

        // 从这里开始，任何失败都必须先收拾。`opened` 是唯一的收拾清单，只有一条清理路径。
        let mut opened = vec![writer.clone()];

        let reader = match open_reader(cfg, cancel, &writer).await {
            Ok(reader) => reader,
            Err(err) => {
                close_all(opened).await;
                return Err(err);
            }
        };
        opened.push(reader.clone());

        let facade = Arc::new(SqliteStorage::new(reader.clone()));

        // 自检走的就是 `/readyz` 会走的那条路——`health()` 本身。另写一条 `SELECT 1` 的话，
        // 自检通过而探活失败就成了可能，那是最难查的一类"启动是好的、上线就红"。
        if let Err(err) = under_cancel(cancel, "self-check", facade.health()).await {
            close_all(opened).await;
            return Err(err);
        }

        Ok((Self { writer, reader }, facade))
    }

    /// 关闭两个池，最迟到 `deadline`。
    ///
    /// 关闭顺序是**先 reader 后 writer**。WAL 的收尾（checkpoint 并删掉 `-wal` / `-shm`）
    /// 由最后一条离开的连接做，而它需要写权限——只读连接做不了。让写池最后出门，收尾才有人做。
    ///
    /// 返回值里**不会**出现 [`CloseOutcome::SkippedUnproven`]：那个结论的前提是"举不出
    /// 没人再用它的证据"，而证据在 `app` 手里（所有任务是否都退出了）。`app` 判断举不出来时
    /// 根本不会调用本方法，直接报 `SkippedUnproven`。
    ///
    /// 到点未关完 → [`CloseOutcome::timed_out`]。注意这时池**不是**关上了：sqlx 的
    /// `close()` 会等所有借出去的连接归还，到点没等到就说明还有人在用。措辞由 `core` 统一
    /// （"close did not complete before its deadline"），这里不另造一句。
    pub async fn close(self, deadline: Instant) -> CloseOutcome {
        let at = tokio::time::Instant::from_std(deadline);
        let closing = async {
            self.reader.close().await;
            self.writer.close().await;
        };

        match tokio::time::timeout_at(at, closing).await {
            Ok(()) => CloseOutcome::Closed,
            Err(_elapsed) => CloseOutcome::timed_out(),
        }
    }
}

/// 迁移 + 建读池。抽出来只是为了让 `open` 的清理路径只有一条。
async fn open_reader(
    cfg: &StorageConfig,
    cancel: &CancellationToken,
    writer: &SqlitePool,
) -> Result<SqlitePool, StorageError> {
    under_cancel(cancel, "migrate", run_migrations(writer)).await?;
    under_cancel(cancel, "connect-reader", sqlite::reader_pool(cfg)).await
}

/// 跑迁移。走写池：迁移是写操作，而写池只有一条连接，所以迁移天然是串行的。
async fn run_migrations(writer: &SqlitePool) -> Result<(), StorageError> {
    MIGRATOR.run(writer).await.map_err(error_map::from_migrate)
}

/// 把一个步骤放到取消之下。
///
/// `biased` 让取消分支永远先被轮询。没有它的话 `select!` 会随机挑一个分支先看，于是
/// "收到 Ctrl-C 之后还多跑了一步"变成一个偶发行为——而偶发的关停行为是查不出来的。
async fn under_cancel<T, F>(
    cancel: &CancellationToken,
    step: &'static str,
    fut: F,
) -> Result<T, StorageError>
where
    F: Future<Output = Result<T, StorageError>>,
{
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(cancelled_at(step)),
        out = fut => out,
    }
}

/// 启动期被取消。
///
/// 用 `Internal` 而不是给 `StorageError` 新加一个 `Cancelled` 变体：新变体会让每一个
/// `match` 多一条臂，而能产生它的只有 `open` 这一个函数，`app` 对它的处理又和别的启动失败
/// 完全一样（记录、退出）。代价是分类上略粗，补偿是 `context` 里带着是哪一步被打断的。
fn cancelled_at(step: &'static str) -> StorageError {
    StorageError::Internal {
        context: format!("open was cancelled during {step}").into_boxed_str(),
    }
}

/// 反序关掉已经建好的池。
///
/// 不设超时、也不和取消赛跑：走到这里时**一条连接都还没借出去过**，`close()` 只需要清掉
/// 池里的空闲连接，不会等任何人。加一个超时只会让"启动失败"这条路径多一种结局。
async fn close_all(pools: Vec<SqlitePool>) {
    for pool in pools.into_iter().rev() {
        pool.close().await;
    }
}

/// 把库文件准备好：父目录存在、文件存在、权限收到只有属主可读写。
///
/// 为什么不交给 `create_if_missing(true)`：驱动建出来的文件模式由 umask 决定，多数环境是
/// 0644，于是"数据库只有属主能读"这条纪律会因为一个默认值而静默失效。
fn prepare_file(path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|_| internal("cannot create the directory holding the database file"))?;
    }

    let existed = path.exists();
    if !existed {
        create_owner_only(path)?;
    }

    #[cfg(unix)]
    if existed {
        tighten_existing(path)?;
    }

    Ok(())
}

/// 新建一个空文件。长度为 0 的文件就是一个合法的空 SQLite 库，驱动连上去会自己初始化。
#[cfg(unix)]
fn create_owner_only(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        // `mode` **只在创建时**生效，正好是这里要的语义。
        .mode(0o600)
        .open(path)
        .map(drop)
        .map_err(|_| internal("cannot create the database file"))
}

/// 非 Unix 上没有这套权限位。本模板**没有**在 Windows 上验证过，所以这里不假装做了什么。
#[cfg(not(unix))]
fn create_owner_only(path: &Path) -> Result<(), StorageError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(drop)
        .map_err(|_| internal("cannot create the database file"))
}

/// 已存在的库文件：**只收紧，不放宽**。
///
/// 清掉组位和其他位，属主位原样保留。直接 `set_permissions(0o600)` 也能达到目的，但那会把
/// 运维刻意设的属主位（比如去掉写位做只读演练）一起改掉——一个启动步骤不该有这种副作用。
#[cfg(unix)]
fn tighten_existing(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt as _;

    let meta =
        fs::metadata(path).map_err(|_| internal("cannot stat the existing database file"))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        return Ok(());
    }

    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o700))
        .map_err(|_| internal("cannot tighten permissions on the existing database file"))
}

/// `Internal` 在本模块的唯一构造点。
fn internal(context: &'static str) -> StorageError {
    StorageError::Internal {
        context: Box::from(context),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use service_testkit::TempDb;
    use sqlx::Row as _;

    use super::*;

    fn config_at(db: &TempDb) -> StorageConfig {
        StorageConfig {
            path: db.path().to_path_buf(),
            max_readers: 2,
            busy_timeout: Duration::from_millis(200),
            acquire_timeout: Duration::from_secs(2),
        }
    }

    fn sidecars(db: &TempDb) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = db.path().as_os_str().to_owned();
        let mut wal = base.clone();
        wal.push("-wal");
        let mut shm = base;
        shm.push("-shm");
        (wal.into(), shm.into())
    }

    #[tokio::test]
    async fn open_creates_the_file_and_the_directory_above_it() {
        let db = TempDb::new().expect("临时库");
        let mut cfg = config_at(&db);
        // 把库塞进一层还不存在的子目录：首次部署就是这个形状。
        cfg.path = db
            .path()
            .parent()
            .expect("父目录")
            .join("data")
            .join("s.sqlite3");
        assert!(!cfg.path.exists());

        let (owner, storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("首次打开");
        assert!(cfg.path.is_file(), "库文件没建出来");
        storage.health().await.expect("刚建好的库是健康的");

        assert!(
            owner
                .close(Instant::now() + Duration::from_secs(5))
                .await
                .is_clean()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_database_and_both_sidecars_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let (owner, storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("首次打开");

        // 探活会读一次，确保 WAL 的边车文件确实被建出来了。
        storage.health().await.expect("健康");

        let (wal, shm) = sidecars(&db);
        let cases: &[(&str, &Path)] = &[
            ("db", db.path()),
            ("wal", wal.as_path()),
            ("shm", shm.as_path()),
        ];
        for (i, (name, path)) in cases.iter().enumerate() {
            assert!(path.is_file(), "TC{i} ({name}) 不存在：{}", path.display());
            let mode = fs::metadata(path).expect("stat").permissions().mode() & 0o777;
            // 边车文件在**创建时继承主库文件的模式**，所以它们不需要单独 chmod——这条用例
            // 就是那个结论的守卫。哪天 SQLite 改了这个行为，这里会红。
            assert_eq!(
                mode & 0o077,
                0,
                "TC{i} ({name}) 的组位/其他位没清干净：{mode:o}"
            );
        }

        owner.close(Instant::now() + Duration::from_secs(5)).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_existing_loose_file_is_tightened_but_owner_bits_are_kept() {
        use std::os::unix::fs::PermissionsExt as _;

        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        fs::write(db.path(), b"").expect("先摆一个文件");
        fs::set_permissions(db.path(), fs::Permissions::from_mode(0o644)).expect("设松权限");

        let (owner, _storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("打开既有库");

        let mode = fs::metadata(db.path()).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "0644 应当被收到 0600，实际 {mode:o}");

        owner.close(Instant::now() + Duration::from_secs(5)).await;
    }

    #[tokio::test]
    async fn migration_set_creates_no_business_tables() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let (owner, _storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("首次打开");

        // **绕开门面**直连原库核对表清单：门面上只有 `health()`，用它证不了这件事。
        let probe = sqlite::reader_pool(&cfg).await.expect("另开一条只读连接");
        let tables: Vec<String> =
            sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&probe)
                .await
                .expect("读表清单")
                .iter()
                .map(|row| row.get::<String, _>(0))
                .collect();
        probe.close().await;

        // 这条断言会**主动过期**：第一条业务迁移落地时它会红。那时把期望值改成对应的表清单
        // ——红在这里，比"模板自带的表悄悄进了生产库"好。
        assert_eq!(
            tables,
            vec!["_sqlx_migrations".to_owned()],
            "模板自带的迁移集不该建任何业务表"
        );

        owner.close(Instant::now() + Duration::from_secs(5)).await;
    }

    #[tokio::test]
    async fn opening_twice_is_idempotent() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);

        for round in 0..2 {
            let (owner, storage) = StorageOwner::open(&cfg, &CancellationToken::new())
                .await
                .unwrap_or_else(|err| panic!("第 {round} 轮打开失败：{err}"));
            storage.health().await.expect("健康");
            assert!(
                owner
                    .close(Instant::now() + Duration::from_secs(5))
                    .await
                    .is_clean(),
                "第 {round} 轮关闭不干净"
            );
        }
    }

    #[tokio::test]
    async fn a_cancelled_open_says_which_step_it_stopped_at() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = StorageOwner::open(&cfg, &cancel)
            .await
            .expect_err("已取消的令牌不该让 open 成功");
        assert_eq!(err.kind_str(), "internal");
        let text = err.to_string();
        assert!(text.contains("cancelled"), "没说清是被取消的：{text}");
        // `biased` 保证第一步就停：不写 `biased` 的话这里偶尔会停在更后面的步骤。
        assert!(text.contains("connect"), "应当停在第一步：{text}");
    }

    #[tokio::test]
    async fn a_modified_migration_fails_fast_and_is_not_repaired() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);

        let (owner, _storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("首次打开");
        owner.close(Instant::now() + Duration::from_secs(5)).await;

        // 伪造"已应用的迁移被改过"：直接把校验和写坏。
        let writer = sqlite::writer_pool(&cfg).await.expect("写池");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
            .execute(&writer)
            .await
            .expect("改校验和");
        writer.close().await;

        let err = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect_err("校验和不符必须失败");
        assert_eq!(err.kind_str(), "migration");
        let text = err.to_string();
        assert!(text.contains("validate"), "阶段应当是 validate：{text}");
        assert!(
            text.contains("_sqlx_migrations"),
            "提示必须说清去哪儿看：{text}"
        );

        // **不自动修复**：坏的校验和还在。
        let probe = sqlite::reader_pool(&cfg).await.expect("只读连接");
        let checksum: Vec<u8> =
            sqlx::query("SELECT checksum FROM _sqlx_migrations WHERE version = 1")
                .fetch_one(&probe)
                .await
                .expect("读回校验和")
                .get(0);
        probe.close().await;
        assert_eq!(checksum, vec![0_u8], "启动失败时不该顺手把数据改回去");
    }

    #[tokio::test]
    async fn a_failed_open_leaves_no_live_pool() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);

        let (owner, _storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("首次打开");
        owner.close(Instant::now() + Duration::from_secs(5)).await;

        let writer = sqlite::writer_pool(&cfg).await.expect("写池");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
            .execute(&writer)
            .await
            .expect("改校验和");
        writer.close().await;

        StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect_err("这一次打开必须失败");

        // 证据是**外部可见**的：WAL 的边车文件只在有连接挂着的时候存在，最后一条连接关掉时
        // 被删。它们还在，就说明 `open` 失败之后还留着活着的池。
        let (wal, shm) = sidecars(&db);
        assert!(!wal.exists(), "失败之后 -wal 还在：{}", wal.display());
        assert!(!shm.exists(), "失败之后 -shm 还在：{}", shm.display());
    }

    #[tokio::test]
    async fn close_past_its_deadline_is_reported_as_failure_not_as_closed() {
        let db = TempDb::new().expect("临时库");
        let cfg = config_at(&db);
        let (owner, _storage) = StorageOwner::open(&cfg, &CancellationToken::new())
            .await
            .expect("打开");

        // 借一条读连接并且**不还**。sqlx 的 `close()` 会等所有借出去的连接归还（sqlx-core
        // 0.9.0 `Pool::close` 的文档原话），所以这条路径是确定的，不是靠掐时间碰运气。
        // 借的是 owner 自己的池——用例是本模块的子模块，私有字段看得见。
        let held = owner.reader.acquire().await.expect("借一条连接");

        let outcome = owner
            .close(Instant::now() + Duration::from_millis(50))
            .await;

        match outcome {
            CloseOutcome::Failed(err) => {
                // 措辞由 `core::CloseOutcome::timed_out()` 统一，这一层不另造一句。
                assert!(
                    err.to_string().contains("deadline"),
                    "到点未关完的措辞应当来自 core：{err}"
                );
            }
            other => panic!("还有连接借在外面时不该报 {}", other.as_str()),
        }

        drop(held);
    }
}
