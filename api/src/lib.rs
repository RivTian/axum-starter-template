//! # `{{crate_prefix}}-api`
//!
//! HTTP 展示层。无业务逻辑，只做展示与装配；数据一律经 [`AppState`] 注入的
//! 门面获取。
//!
//! ## 窄门面（冻结语义）
//!
//! 对外只有四样：[`build_router`]、[`bind`]、[`serve`]、[`AppState`]。
//! handler / error / response 模块一律不出 crate——外部只能经 HTTP 调用端点，测试走
//! oneshot 黑盒；响应契约由 crate 内单测钉死，不构成代码级 API。
//!
//! ## 三条响应契约
//!
//! - 成功带数据 → **裸 JSON**，不套信封（`ApiResponse::Data`）；
//! - 成功无数据 → `{status: "success", code: 200, description: ""}`（`ApiResponse::Ok`）；
//! - 所有错误 → 同形信封，`status: "error"`，`code` 与 HTTP 状态码一致（`HttpError`）。
//!
//! 三条各有单测钉在 `response.rs` / `error.rs` 里。加端点时照抄返回类型
//! `Result<ApiResponse<T>, HttpError>` 就不会跑偏。
//!
//! 第三条要成立还差一步：请求体 / 查询串 / 路径参数解析失败发生在 handler **之前**，
//! 由提取器直接变成响应。**取参一律用 `crate::extract` 下的 `Json` / `Query` / `Path`**，
//! 不要用 `axum::` 下的同名类型——后者的拒绝是一行 `text/plain`，不是信封。
//!
//! ## 依赖边界
//!
//! `api -> core + storage`；不落库细节（拿到的是 `Arc<dyn Storage>`）、不建常驻任务
//! （[`serve`] 返回 future，由 app 决定 spawn 到哪个 runtime 并注册进监管器）、
//! 不初始化 tracing。
//!
//! ## 两个 layer，不建 middleware 目录
//!
//! `TraceLayer`（请求级日志）与 `TimeoutLayer`（悬挂请求不能拖死优雅排空）内联在
//! 路由装配处。出现第三个真实 layer 再谈目录。

mod error;
// 模板自带的三个端点都不取参，包装器因此暂无调用者；它们在这里是为了让**第一个**
// 取参的 handler 默认落在信封契约里，而不是等破了契约才发现（模块头有说明）
#[allow(dead_code)]
mod extract;
mod handler;
mod response;
mod state;

pub use state::AppState;

use std::time::Duration;

use axum::Router;
use axum::extract::OriginalUri;
use axum::http::{Method, StatusCode};
use axum::routing::any;
use tokio_util::sync::CancellationToken;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use {{crate_prefix_snake}}_core::AppResult;
use {{crate_prefix_snake}}_core::config::HttpConfig;

/// 组装完整的 axum `Router`。
///
/// **所有端点都在 `/v1` 下，探针也不例外**：版本面之外再留一片无版本路径，等于
/// 给自己开了第二套契约（第二个未命中兜底、第二处响应形状），而探针路径本来就是
/// 编排器配置里的一个字符串，跟着版本走没有额外成本。路由按组 merge 后统一
/// `nest("/v1")`：加一组端点 = 加一个 handler 子模块 + 在这里补一行 merge，既有组不动。
///
/// # 装配顺序不是随手排的（axum 0.8 路由语义）
///
/// - `v1.fallback(json_404)`：被嵌套的 Router 自带 fallback 时，外层 fallback 不再
///   被它继承——`/v1/**` 未命中必须是 JSON 信封，不能掉进 axum 默认的空 404；
/// - `route("/v1/", …)`：嵌在 `/v1` 的 Router 匹配 `/v1` 而**不匹配** `/v1/`，
///   末端通配也不匹配空串，`/v1/` 这一个路径两头都不中，要显式按回 json_404；
/// - layer 后加的在外层：请求先过 `TraceLayer` 再过 `TimeoutLayer` 再进路由，
///   超时也会被 trace 记到。
pub fn build_router(state: AppState) -> Router {
    let request_timeout = Duration::from_millis(state.config.current().http.request_timeout_ms);
    let v1 = handler::system::router();

    Router::new()
        .nest("/v1", v1.fallback(json_404))
        .route("/v1/", any(json_404))
        // 超时回 408：客户端能分辨「服务没答」与「服务答了个错」
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// `/v1/**` 未命中的兜底：JSON 信封 404，绝不是空响应。
///
/// 走 [`error::HttpError::NotFound`] 而不是自己拼一个响应体：状态码与信封
/// 的对应关系集中在 `error.rs` 一处，另起炉灶迟早两边跑偏。
///
/// 回显方法与路径是刻意的——404 只说「没找到」而不说没找到什么，排查时
/// 等于没说。内容是调用方自己发来的，不引入新信息。
async fn json_404(method: Method, OriginalUri(uri): OriginalUri) -> error::HttpError {
    error::HttpError::NotFound(format!("no route for {method} {}", uri.path()))
}

/// 同步绑定监听端口（bind 失败即中止启动）。
///
/// 用 `std::net` 而不是 `tokio::net`：tokio 的 IO 资源在**创建时**注册到当前
/// runtime 的驱动，而 HTTP 面可能被 app 绑到附加 runtime 上——socket 必须在
/// 那个 runtime 里完成注册（见 [`serve`]）。同步 bind 留在 boot 里，让端口被占、
/// 地址非法这类部署错误在启动期原样上抛，吞掉或重试只会把启动问题拖成运行时问题。
pub fn bind(cfg: &HttpConfig) -> AppResult<std::net::TcpListener> {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = std::net::TcpListener::bind(&addr)?;
    // tokio 要求交给它的 std listener 已是非阻塞模式，否则 accept 会卡住 worker 线程
    listener.set_nonblocking(true)?;
    tracing::info!(%addr, "HTTP listener bound");
    Ok(listener)
}

/// HTTP 服务主体：在目标 runtime 上完成 socket 注册，然后 serve 到取消。
///
/// 返回 future 而不是 `JoinHandle`：spawn 到哪个 runtime 由 app 决定。
/// 顶层任务注册顺序里它必须**最后**启动——HTTP 开始接流量时，其余运行面
/// 必须已经就绪，否则请求会打到半装配的运行面（既定不变量，装配处同款注释）。
pub fn serve(
    state: AppState,
    listener: std::net::TcpListener,
    shutdown: CancellationToken,
) -> impl Future<Output = ()> + Send + 'static {
    let app = build_router(state);
    async move {
        // from_std 在这里而不是 bind 里：注册到的是**当前**（被 spawn 到的）runtime
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(error = %error, "HTTP listener registration failed");
                return;
            }
        };
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        {
            tracing::error!(error = %error, "HTTP service exited with error");
        }
    }
}
