//! 顶层任务面（plane）的类型与监督者。
//!
//! 「面」指的是进程里**长期驻留、可以独立失败**的那几个东西：HTTP 面、周期任务面、
//! 配置重载面。它们和普通的 `tokio::spawn` 出来的短任务不是一回事——短任务失败通常只影响
//! 一个请求，而一个面退出意味着进程失去了一整项能力。
//!
//! 这个模块的全部设计围绕一句话：**面的退出必须是可归因、可分类、可穷尽的。**
//!
//! - 可归因：[`RuntimeId`] 只能从 `Handle` 派生，`TaskName` 是枚举不是字符串。
//! - 可分类：[`ExitKind`] 四分且无 `_` 臂。
//! - 可穷尽：[`TaskSupervisor`] 的两段收割对每个登记过的面都产出一条记录——要么是
//!   [`TaskExit`]，要么是 [`UnreapedTask`](crate::shutdown::UnreapedTask)，没有第三种下场。
//!
//! 另有一件与退出对称的事也放在这里：[`ack_channel`] 是面**进入**服务状态的那一步。
//! 「准备完成」和「已退出」是同一条时间线的两端，让它们共用一套面标识（[`TaskName`]）比
//! 分散在两个模块里更难写错。

mod ack;
mod exit;
mod name;
mod runtime;
mod spec;
mod supervisor;

pub use ack::{AckReceiver, AckSender, ack_channel};
pub use exit::{ExitKind, PanicSummary, PlaneError, TaskExit};
pub use name::TaskName;
pub use runtime::RuntimeId;
pub use spec::{ShutdownClass, TaskSpec};
pub use supervisor::{HarvestOutcome, RegisterError, TaskSupervisor};

use std::future::Future;
use std::pin::Pin;

/// 一个顶层任务面的 future 类型。
///
/// 输出是 `Result<(), PlaneError>` 而不是 `()`：这一条是类型级的纪律。`Output = ()` 会让
/// 「HTTP 面起不来」和「HTTP 面正常收尾」在 `JoinSet` 的产出里变成同一个形状，关停报告
/// 因此永远说不清发生了什么，退出码也只能一律是 0。
///
/// 用装箱 future 而不是把 `TaskSupervisor` 泛型化到 future 类型上：supervisor 要在一个
/// `JoinSet` 里放下形状各异的面，泛型参数做不到这件事。每个面装箱一次——全进程三次分配。
pub type PlaneFuture = Pin<Box<dyn Future<Output = Result<(), PlaneError>> + Send>>;
