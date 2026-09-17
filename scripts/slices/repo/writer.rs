//! 切片二：仓储的消费者（任务面）。
//!
//! 仓储不能只是"写好了没人用"——这里演示它被一个任务面消费：启动时写一条，之后等取消。

use svc_core::Error;
use svc_runtime::TaskContext;
use svc_storage::NotesRepo;

/// 写一条笔记并汇报条数，然后等取消。
pub async fn run(ctx: TaskContext, repo: NotesRepo) -> Result<(), Error> {
    let id = repo.insert("hello from the repo slice").await?;
    let count = repo.count().await?;
    tracing::info!(id, count, task = %ctx.key(), "notes written");
    ctx.cancelled().await;
    Ok(())
}
