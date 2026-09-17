//! 切片一：第一个顶层任务面。
//!
//! 契约（来自 crates/runtime）：任务面**返回 future**，spawn 到哪个 runtime 由装配层决定；
//! 收尾必须观察 `ctx.cancelled()`——否则关停时它只能被 abort，报告里会记成"没收回来的任务"。

use std::time::Duration;

use svc_core::Error;
use svc_runtime::TaskContext;

/// 每 5 秒打一条心跳；收到取消就收尾返回。
pub async fn run(ctx: TaskContext) -> Result<(), Error> {
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = ctx.cancelled() => {
                tracing::info!(task = %ctx.key(), "flush stopped");
                return Ok(());
            }
            _ = ticker.tick() => {
                tracing::info!(task = %ctx.key(), "flush tick");
            }
        }
    }
}
