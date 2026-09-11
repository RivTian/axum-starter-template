//! `{{crate_prefix}}-reconcile` —— 期望集 reconcile 框架（可选）。
//!
//! 解决的是一个通用形状：**一组由配置或库表决定的长活单元，配置变了就关旧
//! 建新、死了就退避重建、反复死就放弃、超限就推迟**。每租户 worker、每设备
//! 会话、每队列消费者都是这个形状。需要它的面才引它。
//!
//! # 默认没有人依赖它
//!
//! 它是 workspace 成员（永远参与 `cargo build` 与 `cargo test`），但
//! `{{crate_prefix}}-app` 的依赖树里不含它——示例面 `ticker` 是单个常驻循环，
//! 不需要这一层。接入时在用它的那个面级 crate 的 `[dependencies]` 里加
//! `{{crate_prefix}}-reconcile = { workspace = true }`，并在评审说明这条新边。
//!
//! # 两层监管的分工
//!
//! `{{crate_prefix}}-core::task::TaskSupervisor` 管**顶层任务**（每个面一个，
//! 进程级 first-failure）；本 crate 管**面内部的一组单元**。一个面接入本框架
//! 之后仍然只在 supervisor 里占一个名额：面的顶层任务就是那条 [`drive`] 循环。
//!
//! # 四个文件的分工（照这个顺序读）
//!
//! | 文件 | 职责 | 形态 |
//! | --- | --- | --- |
//! | `unit` | 契约：[`UnitSpec`] / [`UnitFactory`] / [`UnitCtx`] / [`UnitExit`] | 纯 trait |
//! | `plan` | 决策：期望集 + 现状 → 动作清单 | 纯函数，同步可测 |
//! | `manager` | 执行与记账：spawn / 取消 / 收割 / 死亡计数 | 有运行时，单写者无锁 |
//! | `driver` | 驱动循环：事件唤醒 + 定时兜底 | 有运行时，模拟时钟测 |
//!
//! # 怎么接一个面
//!
//! 1. 定义 `Spec`（配置或库表行的纯数据投影），实现 [`UnitSpec`]：`key` 是身份，
//!    `same_as` 只比「改了就必须重建」的字段；
//! 2. 定义工厂，持有单元需要的依赖（存储句柄、配置读端、事件总线），实现
//!    [`UnitFactory::spawn_unit`]：返回单元主循环，第一分支 `ctx.cancel.cancelled()`，
//!    干成事就 [`UnitCtx::mark_progress`]；
//! 3. 定义来源，实现 [`UnitSource`]：`list_desired` 读期望集，`tick_interval` 给
//!    兜底巡检周期；
//! 4. 面的顶层任务体里建 [`UnitManager`]，跑 [`drive`]（唤醒谓词常规传
//!    [`wake_on_plane`]），循环返回后走 [`UnitManager::shutdown`]；
//! 5. 期望集的写入方在写成功后 `publish(AppEvent::DesiredSetChanged { plane })`，
//!    让变更提前一拍生效。事件丢了只是慢一拍，兜底巡检是真值。

mod driver;
mod manager;
mod plan;
mod unit;

pub use driver::{DriveOptions, UnitSource, drive, wake_on_plane};
pub use manager::{ReconcileReport, UnitManager};
pub use plan::{ReconcilePlan, RestartPolicy, RestartReason, UnitView, plan_reconcile};
pub use unit::{UnitCtx, UnitExit, UnitFactory, UnitSpec, UnitStatus};
