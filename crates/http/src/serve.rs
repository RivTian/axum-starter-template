//! HTTP 任务面：零路由起步的 `router()` 与可关停的 `serve(...)`。
//!
//! 骨架故意没有业务端点，也没有 `/healthz`：就绪语义需要一个"就绪来源"，而骨架里没有任何消费者
//! （没有编排器、没有依赖探针），预置就变成假实现。加第一条路由时把健康检查一起加上即可。

use std::future::Future;

use axum::Router;
use {{crate_prefix_snake}}_core::{Error, ErrorKind};
use tokio::net::TcpListener;

/// 零路由起步的 router。加第一条路由从这里开始（`Router::route(...)`）。
pub fn router() -> Router {
    Router::new()
}

/// 任务面：把 `router` 服务到 `shutdown` 完成。
///
/// `shutdown` 是 `ctx.cancelled()`（装配层传进来）：它解析后 axum 停止接收新连接并等待
/// 在途连接收尾。返回 `Ok(())` 表示正常停服；`Err` 表示服务本身异常退出（会被 supervisor
/// 记成任务失败）。
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), Error> {
    let address = listener
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "<unknown>".to_owned());
    tracing::info!(address = %address, "http listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|err| Error::with_source(ErrorKind::Task, "HTTP 服务异常退出", err))
}
