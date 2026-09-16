//! sqlx 的错误 → `core::StorageError` 的归一化。
//!
//! 这个模块是「门面不泄漏后端」的收口处：它是**唯一**知道 `sqlx::Error` 长什么样的地方，
//! 出去的一律是 `core` 的类型。
//!
//! 它写成自由函数而不是 `impl From<sqlx::Error> for StorageError`，原因不是风格：
//! 孤儿规则不允许——`sqlx::Error` 与 `StorageError` 对本 crate 都是外部类型。这条限制
//! 在这里恰好是好事，它把"谁能构造 `StorageError`"钉死在了实现层。
//!
//! # 落进 `Internal { context }` 的字符串
//!
//! 一律是**本文件里的字面量**，或者字面量加一个 SQLite 结果码（一个整数）。不把上游错误
//! `to_string()` 进去——sqlx 的部分 `Display` 实现会带上语句片段，而 `StorageError` 的
//! 契约是「不含 SQL、不含凭据」。代价是排查时少一层细节，补偿是 `hint` / `context` 都写成
//! 「去哪儿看」而不是「出了什么事」。
//!
//! 措辞用英文：它们最终出现在日志里。

use service_core::storage::{MigrationStage, StorageError};
use sqlx::error::{DatabaseError, ErrorKind};

use crate::sqlite::BACKEND;

/// SQLite **主**结果码。`DatabaseError::code()` 返回的是扩展码（例如 1555 =
/// `SQLITE_CONSTRAINT_PRIMARYKEY`），低 8 位才是主码，所以比较前要先掩一下。
const PRIMARY_CODE_MASK: i32 = 0xFF;
const SQLITE_BUSY: i32 = 5;
const SQLITE_LOCKED: i32 = 6;
const SQLITE_READONLY: i32 = 8;

/// 把任意 `sqlx::Error` 归一化。
pub(crate) fn from_sqlx(err: sqlx::Error) -> StorageError {
    // 数据库自己报的错单独走一条路：它带结果码与约束信息，是唯一能区分
    // `NotFound` / `Conflict` / `UniqueViolation` 的来源。
    if let Some(db) = err.as_database_error() {
        return from_database(db);
    }

    match err {
        // 查询本身成立，只是没有行。
        sqlx::Error::RowNotFound => StorageError::NotFound,

        sqlx::Error::Migrate(inner) => from_migrate(*inner),

        // 池排队超时 / 池已关 / 连接 I/O 断了 / 后台任务挂了：四种的结论对调用方是同一个
        // ——现在连不上。`PoolClosed` 实际上多半是我们自己的时序 bug（关了还在用），但把它
        // 单列成 `Internal` 会让停机窗口里的健康检查报 500 而不是 503，那是更坏的谎。
        sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::WorkerCrashed => StorageError::Unavailable { backend: BACKEND },

        sqlx::Error::Configuration(_) | sqlx::Error::ConfigFile(_) => {
            internal("sqlx configuration is invalid")
        }
        sqlx::Error::Protocol(_) => internal("sqlite driver protocol error"),
        sqlx::Error::TypeNotFound { .. }
        | sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::Decode(_) => {
            internal("decoding a row failed: the query and the Rust type disagree")
        }
        sqlx::Error::ColumnIndexOutOfBounds { .. } | sqlx::Error::ColumnNotFound(_) => {
            internal("a column named in the query is absent from the result")
        }
        sqlx::Error::Encode(_) => internal("encoding a bind parameter failed"),
        sqlx::Error::InvalidArgument(_) => internal("a query was built with an invalid argument"),
        sqlx::Error::BeginFailed | sqlx::Error::InvalidSavePointStatement => {
            internal("opening a transaction failed")
        }

        // `sqlx::Error` 是 `#[non_exhaustive]`：这条臂删不掉。升级 sqlx 后新变体会静默走到
        // 这里，所以措辞里写明"未分类"，让日志能把它和上面那些区分开。
        _ => internal("unclassified sqlite driver error"),
    }
}

/// 把迁移错误归一化。
///
/// 三个阶段的划分对应 `MigrationStage`：读迁移集（`Load`）、校验已应用记录（`Validate`）、
/// 执行（`Apply`）。`hint` 是 `&'static str`，带不出版本号——所以它一律写成「去哪张表看」。
pub(crate) fn from_migrate(err: sqlx::migrate::MigrateError) -> StorageError {
    use sqlx::migrate::MigrateError as M;

    let (stage, hint) = match err {
        // 迁移集是编译期嵌入的，所以这条在运行期几乎摸不到。留着不是为了"以防万一"：
        // `MigrateError` 是 `#[non_exhaustive]`，兜底臂总要有一个，而把这个变体并进兜底
        // 会让它报成"未分类"——明明知道是哪一类还报未分类，是给排障的人添乱。
        M::Source(_) => (
            MigrationStage::Load,
            "the embedded migration set could not be resolved; \
             check the files under migrations/ — names must look like <VERSION>_<DESCRIPTION>.sql",
        ),

        // 内容被改过。迁移只增不改，所以这是一条纪律问题，不是数据问题。
        M::VersionMismatch(_) => (
            MigrationStage::Validate,
            "an applied migration was modified; \
             compare checksums in _sqlx_migrations against the files under migrations/",
        ),

        // 库里记着一条本地不存在的迁移：已发布的迁移文件被删了，或者部署了更老的版本。
        M::VersionMissing(_) | M::VersionNotPresent(_) => (
            MigrationStage::Validate,
            "_sqlx_migrations records a version that is absent from the binary; \
             released migrations must not be deleted, and the binary must not be rolled back \
             past an applied migration",
        ),

        M::VersionTooOld(..) | M::VersionTooNew(..) => (
            MigrationStage::Validate,
            "migration versions are out of order; compare the file prefixes under migrations/ \
             with the applied versions in _sqlx_migrations — versions must increase monotonically",
        ),

        // 上一次迁移中途挂了。**不自动修复**：在没人看着的时候改数据库比启动失败危险得多。
        M::Dirty(_) => (
            MigrationStage::Validate,
            "a previous migration was interrupted; inspect the unfinished row in \
             _sqlx_migrations, fix the schema by hand, remove that row, then restart",
        ),

        M::Execute(_) | M::ExecuteMigration(..) => (
            MigrationStage::Apply,
            "a migration statement failed; the last row of _sqlx_migrations points at which one",
        ),

        // `MigrateError` 同样是 `#[non_exhaustive]`，而且里面有一个 `#[deprecated]` 变体
        // ——显式匹配它会因为 `-D warnings` 直接判红，所以它只能走这条臂。
        _ => (
            MigrationStage::Validate,
            "unclassified migration failure; inspect _sqlx_migrations",
        ),
    };

    StorageError::Migration { stage, hint }
}

/// 数据库自己报的错。
fn from_database(db: &dyn DatabaseError) -> StorageError {
    match db.kind() {
        ErrorKind::UniqueViolation => StorageError::UniqueViolation {
            constraint: constraint_of(db),
        },

        // 外键 / 非空 / CHECK / 排他：四种都是"这条写入与当前状态不相容"。分开成四个变体
        // 会让 `api` 的状态码映射多三个分支，而这四种在 HTTP 上是同一个 409。
        ErrorKind::ForeignKeyViolation
        | ErrorKind::NotNullViolation
        | ErrorKind::CheckViolation
        | ErrorKind::ExclusionViolation => StorageError::Conflict,

        // `ErrorKind::Other` 以及未来新增的种类。
        _ => from_primary_code(db),
    }
}

/// `ErrorKind::Other` 之下再按 SQLite 结果码分一次。
fn from_primary_code(db: &dyn DatabaseError) -> StorageError {
    match primary_code(db) {
        // 锁竞争。已经等过 `busy_timeout` 了还是拿不到，对调用方就是"现在用不了"。
        Some(SQLITE_BUSY | SQLITE_LOCKED) => StorageError::Unavailable { backend: BACKEND },

        // 写落到了只读池上。这是**我们自己的 bug**，不是环境问题，所以它必须是 `Internal`
        // ——让它变成 503 会让人去查磁盘和负载，而真正要看的是哪段代码拿错了池。
        Some(SQLITE_READONLY) => internal("a write was attempted on the read-only pool"),

        // 结果码是个整数，带进日志是安全的；SQLite 的消息文本不一定安全，所以不带。
        Some(code) => internal(format!("sqlite error code {code}")),
        None => internal("sqlite error without a result code"),
    }
}

/// 取主结果码。
fn primary_code(db: &dyn DatabaseError) -> Option<i32> {
    db.code()?
        .parse::<i32>()
        .ok()
        .map(|c| c & PRIMARY_CODE_MASK)
}

/// 取约束名。
///
/// SQLite 驱动**不实现** `DatabaseError::constraint()`——sqlx 的默认实现返回 `None`，
/// 只有 Postgres 驱动填它（sqlx-core 0.9.0 `src/error.rs` 的 trait 默认实现，以及
/// sqlx-sqlite 0.9.0 `src/error.rs` 里 `impl DatabaseError for SqliteError` 没有这一项）。
/// 于是只能从消息里取：SQLite 的原文形如 `UNIQUE constraint failed: t.c`。
///
/// 取出来的是**库里的标识符**，不是用户输入的值，符合 `StorageError` 的契约。
fn constraint_of(db: &dyn DatabaseError) -> Box<str> {
    if let Some(name) = db.constraint() {
        return Box::from(name);
    }
    db.message()
        .split_once("constraint failed: ")
        .map_or_else(|| Box::from("unknown"), |(_, tail)| Box::from(tail.trim()))
}

/// `Internal` 的唯一构造点。
fn internal(context: impl Into<Box<str>>) -> StorageError {
    StorageError::Internal {
        context: context.into(),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::{Executor as _, SqlitePool};

    use super::*;

    /// 一个单连接的内存库。单连接是必须的：`sqlite::memory:` 每条连接都是**各自独立**的
    /// 一个库，多连接的池会让建表和查询落到不同的库上。
    async fn memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("连内存库")
    }

    #[tokio::test]
    async fn no_rows_maps_to_not_found() {
        let pool = memory_pool().await;
        let err = sqlx::query("SELECT 1 WHERE 0")
            .fetch_one(&pool)
            .await
            .expect_err("这条查询不会有行");
        assert_eq!(from_sqlx(err).kind_str(), "not_found");
    }

    #[tokio::test]
    async fn unique_violation_carries_the_constraint_name() {
        let pool = memory_pool().await;
        pool.execute("CREATE TABLE t (c TEXT UNIQUE)")
            .await
            .expect("建表");
        pool.execute("INSERT INTO t (c) VALUES ('x')")
            .await
            .expect("第一次插入");

        let err = sqlx::query("INSERT INTO t (c) VALUES ('x')")
            .execute(&pool)
            .await
            .expect_err("第二次插入应当撞唯一约束");

        match from_sqlx(err) {
            StorageError::UniqueViolation { constraint } => {
                // 名字来自消息里的 `UNIQUE constraint failed: t.c`。断言它**不是**兜底值，
                // 是因为兜底值会让这条错误在日志里彻底没用。
                assert_eq!(&*constraint, "t.c", "约束名没解析出来");
            }
            other => panic!("期望 unique_violation，实际 {}", other.kind_str()),
        }
    }

    #[tokio::test]
    async fn other_constraint_kinds_collapse_into_conflict() {
        let pool = memory_pool().await;
        // 外键在 SQLite 里默认**关闭**——这正是生产连接要显式打开它的理由。
        pool.execute("PRAGMA foreign_keys = ON")
            .await
            .expect("开外键");
        pool.execute("CREATE TABLE parent (id INTEGER PRIMARY KEY)")
            .await
            .expect("建父表");
        pool.execute("CREATE TABLE child (p INTEGER REFERENCES parent(id))")
            .await
            .expect("建子表");
        pool.execute("CREATE TABLE checked (n INTEGER CHECK (n > 0))")
            .await
            .expect("建 CHECK 表");
        pool.execute("CREATE TABLE required (n INTEGER NOT NULL)")
            .await
            .expect("建 NOT NULL 表");

        let cases: &[(&str, &str)] = &[
            ("foreign_key", "INSERT INTO child (p) VALUES (404)"),
            ("check", "INSERT INTO checked (n) VALUES (0)"),
            ("not_null", "INSERT INTO required (n) VALUES (NULL)"),
        ];
        for (i, (name, sql)) in cases.iter().enumerate() {
            let err = sqlx::query(*sql)
                .execute(&pool)
                .await
                .expect_err("这条写入应当被约束挡住");
            assert_eq!(
                from_sqlx(err).kind_str(),
                "conflict",
                "TC{i} ({name}) 没有归一化成 conflict"
            );
        }
    }

    #[test]
    fn every_migration_stage_is_reachable_and_carries_a_hint() {
        use sqlx::migrate::MigrateError as M;

        // 这里直接构造 `MigrateError`，不去制造真的迁移事故：本条用例要证的是**映射**，
        // 而"真事故能产生这些变体"由 `open.rs` 的用例分别证（那里造的是真的元数据异常）。
        let cases: Vec<(&str, M, MigrationStage)> = vec![
            ("source", M::Source("boom".into()), MigrationStage::Load),
            ("mismatch", M::VersionMismatch(1), MigrationStage::Validate),
            ("missing", M::VersionMissing(1), MigrationStage::Validate),
            ("dirty", M::Dirty(1), MigrationStage::Validate),
            ("too_new", M::VersionTooNew(9, 1), MigrationStage::Validate),
            (
                "execute",
                M::ExecuteMigration(sqlx::Error::PoolClosed, 1),
                MigrationStage::Apply,
            ),
        ];

        for (i, (name, err, want)) in cases.into_iter().enumerate() {
            match from_migrate(err) {
                StorageError::Migration { stage, hint } => {
                    assert_eq!(stage, want, "TC{i} ({name}) 阶段判错");
                    assert!(
                        hint.contains("_sqlx_migrations") || hint.contains("migrations/"),
                        "TC{i} ({name}) 的 hint 没指出去哪儿看：{hint}"
                    );
                }
                other => panic!("TC{i} ({name}) 期望 migration，实际 {}", other.kind_str()),
            }
        }
    }

    #[test]
    fn migrate_errors_nested_in_sqlx_error_are_not_swallowed() {
        // `Migrator::run` 的失败会被包成 `sqlx::Error::Migrate`。漏掉这一层的话，迁移失败
        // 会退化成 `Internal`，`app` 那边就分不出"迁移问题"和"驱动问题"。
        let err = sqlx::Error::Migrate(Box::new(sqlx::migrate::MigrateError::Dirty(1)));
        assert_eq!(from_sqlx(err).kind_str(), "migration");
    }

    #[test]
    fn internal_context_never_embeds_upstream_text() {
        // 守的是 `StorageError` 的契约：`context` 只能是本文件的字面量（或字面量 + 结果码）。
        // 这条用例用一个 `Display` 里带着语句片段的上游错误来试探。
        let err = sqlx::Error::Protocol("SELECT secret FROM users WHERE token = 'abc'".into());
        match from_sqlx(err) {
            StorageError::Internal { context } => {
                assert!(
                    !context.contains("SELECT") && !context.contains("abc"),
                    "上游文本漏进了 context：{context}"
                );
            }
            other => panic!("期望 internal，实际 {}", other.kind_str()),
        }
    }
}
