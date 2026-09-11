//! 任务框架：顶层任务监管。
//!
//! [`TaskSupervisor`] 负责顶层任务（模板里是 `ticker` 与 `http`；真实服务按面加）
//! 的注册、first-failure 感知与关停收敛。二级的「期望集 reconcile」框架
//! （`UnitManager` / `drive`）在可选 crate `{{crate_prefix}}-reconcile` 里，
//! 只有需要「一组由配置决定的长活单元」的面才引它。
//!
//! # 任务收敛三模式（何处用何种）
//!
//! 1. **批量 cancel → 限时收割 → 超时 abort**（任务组）：[`TaskSupervisor`] 与
//!    reconcile 的 `UnitManager::shutdown`——先给宽限期，超时强杀，
//!    绝不留 detached task，也绝不留同 key 双化身；
//! 2. **`select!` on cancel**（循环体）：顶层任务 / 单元主循环
//!    `select! { _ = cancel.cancelled() => break, ... }`，取消是第一分支
//!    （`biased` 时关停优先于新工作）；
//! 3. **`Drop` 兜底 abort**（长活结构）：[`TaskSupervisor`] 的 Drop——
//!    monitor 自身被取消时任务也不脱管。

mod supervisor;

pub use supervisor::{TaskExit, TaskSupervisor};
