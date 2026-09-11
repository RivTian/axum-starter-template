//! 全局运行时状态
//!
//! 由 [`crate::boot::boot_strap`] 一次性装配；除 `ConfigStore` 写端外全部以
//! `Arc`/Clone 注入各面。**进程内没有任何业务全局态**——测试并行化的前提。
//!
//! # 字段按需补齐
//!
//! 仅在出现真实消费者时添加字段，不预放无实义的占位类型。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use {{crate_prefix_snake}}_core::config::ConfigStore;
use {{crate_prefix_snake}}_core::events::EventBus;
use {{crate_prefix_snake}}_core::metrics::Metrics;
use {{crate_prefix_snake}}_storage::Storage;

use crate::rt::Executors;

/// 运行期共享资源集合；生命周期统一归 monitor 管理。
pub struct RuntimeState {
    /// watch 写端（`handle()` 派生只读句柄注入各任务）
    pub config: ConfigStore,
    /// 事件出口：只发通知不载荷；monitor 发 `ConfigReloaded` / `ShuttingDown`
    pub events: EventBus,
    /// 持久化门面；以 trait 对象注入，上层不感知 SQLite/PostgreSQL 差异，
    /// 也拿不到连接池——换后端不会波及任何调用方
    pub storage: Arc<dyn Storage>,
    /// 进程自省计数：app 构造，面写、接口面读——两侧共用的一份内存态，谁都不该自己造一份
    pub metrics: Arc<Metrics>,
    /// 各 runtime 的 Handle：任务注册时决定每个面 spawn 到哪
    pub executors: Executors,
    /// 根令牌；一切任务用 `child_token()`（级联取消）
    pub shutdown: CancellationToken,
}
