//! 存储门面与仓储 trait
//!
//! 上层只见 [`Storage`] 与各仓储 trait，见不到 `SqlitePool` / `PgPool` /
//! 任何 sqlx 类型。这条边界让「换后端」和「加仓储」都不波及调用方。
//!
//! 模板不带任何仓储：仓储 trait 与 [`Storage`] 上的访问器（`fn xxx(&self) -> &dyn XxxRepo`）
//! 随首个真实消费者按垂直切片加入，每加一个都要求 SQLite / PostgreSQL 双实现
//! 同时到位，否则不合并。

use async_trait::async_trait;

use crate::error::StorageResult;
use crate::model::StorageHealth;

/// 存储门面。
///
/// 要求 `Debug`：否则任何持有 `Arc<dyn Storage>` 的结构体都无法 `#[derive(Debug)]`，
/// 而 `Result<Arc<dyn Storage>, _>` 上的 `unwrap`/`expect` 也用不了——这两样在测试和
/// 排障里天天要用。实现侧输出后端名即可，绝不可打印连接串或凭据。
#[async_trait]
pub trait Storage: std::fmt::Debug + Send + Sync {
    /// 后端标识，用于日志与诊断。
    fn backend_name(&self) -> &'static str;

    /// 连通性与容量快照（就绪探针的判据）。
    async fn health(&self) -> StorageResult<StorageHealth>;

    /// 周期性维护（SQLite: WAL checkpoint / optimize；PostgreSQL: 统计信息刷新）。
    /// 由上层调度器按需调用，存储层自身不持有常驻任务。
    async fn run_maintenance(&self) -> StorageResult<()>;

    /// 优雅关闭：等待在途操作结束并释放连接池。
    ///
    /// 不返回 `Result`：关停路径上没有可执行的补救动作，失败只应记日志。
    async fn close(&self);
}
