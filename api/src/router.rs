//! 路由树与中间件栈。
//!
//! # 中间件顺序：写下来的顺序和执行顺序是**反的**
//!
//! axum 的规则（`axum-0.8.9/src/docs/middleware.md`）：后 `.layer()` 的在**外面**，请求
//! 自下而上穿过。所以下面那串 `.layer()` 读起来是从内到外，而它实际长这样：
//!
//! ```text
//! 请求 ─→ TraceLayer ─→ CatchPanicLayer ─→ handler_timeout ─→ DefaultBodyLimit ─→ handler
//!          │              │                 │                  │
//!          │              │                 │                  └ 只设一个 extension，
//!          │              │                 │                    由提取器在最里面读
//!          │              │                 └ 只包住"产出响应头"那段，不包 body 流
//!          │              └ handler panic 变成一个信封，不带走进程
//!          └ 最外层：连被里面任何一层拦掉的请求也会被记下来
//! ```
//!
//! 每一层为什么在那个位置：
//!
//! - **`TraceLayer` 必须最外。** 它要记的是"这次请求发生了什么"，包括被超时拦掉的、被
//!   panic 拦掉的、被 body 上限拒掉的。放在里面的话，恰恰是最该看见的那几类请求不会出现
//!   在日志里。
//! - **`CatchPanicLayer` 在超时外面。** panic 可能发生在超时中间件自己身上（比如以后有人
//!   往里加了逻辑）。反过来放，那一类 panic 就直接穿到 hyper，连接被砍，客户端拿到的是
//!   一次 EOF 而不是 500。
//! - **`handler_timeout` 在 body 上限里面。** 上限是靠 extension 生效的，它必须在提取器
//!   跑之前就装上；而超时要计的是 handler 自己花的时间。
//! - **`DefaultBodyLimit` 最内。** 它只做一件事：往请求上挂一个 extension。挂得越靠近
//!   handler，中间被别人改掉的机会越小。
//!
//! # `layer` 与 `method_not_allowed_fallback` 都只作用于**已经注册**的路由
//!
//! 这是同一个坑的两种形态。`Router::layer` 会同时作用到 `path_router`、`fallback_router`
//! 和 `catch_all_fallback`（`axum-0.8.9/src/routing/mod.rs`），所以 fallback 也被信封和
//! trace 覆盖——**前提是它在 `.layer()` 之前就挂上了**。
//!
//! 因此这个函数里的顺序是死的：**先路由、再 fallback、再 405、最后 layer**。有人把
//! `.route()` 挪到 `.layer()` 后面，新路由会静悄悄地不带任何中间件——不报错、不告警。
//!
//! # 模板里没有 `nest`
//!
//! 「`nest` 内 fallback + 顶层 fallback」两处都挂看起来更稳妥。实测下来内层不需要：
//! 内层路由器**没有**自己的 fallback 时会继承外层的（`axum-0.8.9/src/docs/routing/nest.md`
//! 的 Fallbacks 一节）。所以 `/v1/info` 是一条普通路由，不是一个 nest，也就没有第二个
//! fallback 要写。
//!
//! 真正的风险是反过来的：**你给 `nest` 进去的路由器加了 fallback，外层那条就对整个子树
//! 失效了**，于是信封在那一片消失。这条写在 [`README`](../README.md) 的"加业务路由"一节。

use axum::Router;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use crate::error::HttpError;
use crate::middleware::panic::respond_to_panic;
use crate::middleware::timeout::handler_timeout;
use crate::state::AppState;
use crate::system;

/// 组装整棵路由树。
///
/// `business` 是你的业务路由（默认那份在 [`business`](crate::business) 里）。它**不能带
/// fallback**，而且带了**不会有任何信号**：下面 `.merge(business)` 排在 `.fallback()` 之前，
/// 合并那一刻外层还是默认 fallback，axum 0.8.9 走 `(true, false)` 臂把你的 fallback 收下，
/// 随后那句 `.fallback(not_found)` 把它覆盖掉。不 panic、不告警——见
/// [`business`](crate::business) 模块文档里的实测记录。这条禁令由门禁扫描守。
///
/// # 请求体上限是**半热**的
///
/// `http.body_limit_bytes` 在这里读一次，之后钉在 router 上。热重载改了它不会生效，
/// 要重启面才换——这与配置的热度表一致，也是"配置项按它
/// 真实的覆盖面命名"的另一面：叫 `body_limit_bytes` 而不是 `max_request_size`，是因为
/// 它管的就是这一个数字，不是整条请求链路。
// 这里**没有** `#[must_use]`：`Router` 自己就带着它，再加一个是 `double_must_use`。
pub fn router(state: AppState, business: Router<AppState>) -> Router {
    let body_limit = state.config.load().config().http.body_limit_bytes;

    Router::new()
        // ── 系统端点。三条都不在任何版本前缀下：它们是**进程**的属性，不是 API 的一部分,
        //    给它们带上版本号等于承诺"v2 里可能换一套存活语义"，而那不是真的。
        .route("/healthz", get(system::healthz))
        .route("/readyz", get(system::readyz))
        // `/v1/info` 例外：它报的是这一版 API 背后的构建信息，所以带版本前缀是对的。
        .route("/v1/info", get(system::info))
        // ── 你的路由。放在这里，于是它和系统端点走**同一套**中间件。
        .merge(business)
        // ── 路由树的补集。必须在 `.layer()` 之前挂上，见模块文档。
        .fallback(not_found)
        // ── 路径存在但方法不对。axum 的默认响应是 405 + **空 body**——又一次"第三方层
        //    自带一套出错响应"。不接管的话，信封在这一档上有个洞。
        //    这一行必须在所有 `.route()` / `.merge()` 之后：它只改**已经注册**的那些
        //    `MethodRouter`（axum 的文档原话是 "all previously registered"）。
        .method_not_allowed_fallback(method_not_allowed)
        // ── 中间件。写的顺序是从内到外，见模块文档。
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            handler_timeout,
        ))
        .layer(CatchPanicLayer::custom(respond_to_panic))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// 路由树里没有这条路径。
async fn not_found() -> Response {
    HttpError::not_found().into_response()
}

/// 路径有，方法不对。
async fn method_not_allowed() -> Response {
    HttpError::client(
        axum::http::StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "this path does not accept that method",
    )
    .into_response()
}
