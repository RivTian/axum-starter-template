//! `config-watch` 任务面：监听配置文件、去抖、跑重载事务、把报告打成一条日志。
//!
//! 去抖的正确形态是"先收到事件 → 等一小段 → 再读文件"：读到的永远是当时的完整内容。
//! 如果反过来（先读再等），编辑器"写临时文件 + rename"的两步写入会让我们读到半截文件。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use {{crate_prefix_snake}}_config::Reloader;
use {{crate_prefix_snake}}_core::{Error, ErrorKind};
use {{crate_prefix_snake}}_runtime::TaskContext;

use crate::config_state::ConfigState;

const DEBOUNCE: Duration = Duration::from_millis(250);

/// 一次事件到达后的处理：去抖 → 吸走积压事件 → 重载 → 应用 → 打日志。
pub async fn run(
    ctx: TaskContext,
    mut reloader: Reloader,
    state: Arc<ConfigState>,
    mut inbox: mpsc::UnboundedReceiver<()>,
) -> Result<(), Error> {
    loop {
        tokio::select! {
            _ = ctx.cancelled() => {
                tracing::info!(task = %ctx.key(), "config watch stopping");
                return Ok(());
            }
            event = inbox.recv() => {
                if event.is_none() {
                    return Err(Error::new(ErrorKind::Task, "配置文件监听器已停止"));
                }
            }
        }

        tokio::time::sleep(DEBOUNCE).await;
        while inbox.try_recv().is_ok() {}

        match reloader.reload() {
            Ok(outcome) => match state.apply(outcome) {
                Ok(report) => tracing::info!(
                    applied = ?report.applied,
                    next_use = ?report.next_use,
                    restart_required = ?report.restart_required,
                    "configuration reloaded"
                ),
                Err(err) => tracing::error!(
                    error = %err,
                    "配置重载被拒绝（保留 last-good）：可热段没能应用"
                ),
            },
            Err(err) => tracing::error!(
                error = %err,
                "配置重载失败（保留 last-good）：候选配置不合法"
            ),
        }
    }
}
