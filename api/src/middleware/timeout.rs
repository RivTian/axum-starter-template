//! handler 超时。自己写的，不用 `tower_http::timeout::TimeoutLayer`。
//!
//! # 两条理由，各自独立成立
//!
//! 1. **空 body 绕过信封。** `TimeoutLayer` 超时时产出的是 `408` + 空 body。客户端拿到一个
//!    没有 `error` 短名、没有 `message` 的响应，只能靠状态码猜。而错误响应只该有一种
//!    形状，这一条直接违背。
//! 2. **`TimeoutLayer::new` 自 tower-http 0.6.7 起 `#[deprecated]`。** 本仓库 `-D warnings`，
//!    用了当场红。
//!
//! # 它**不**覆盖响应体的流式推送
//!
//! 被 `timeout` 包住的只有 `next.run(request)`——这个 future 在 handler 返回响应**头**时就
//! 完成了。之后 body 怎么推、推多久，这里管不着。
//!
//! 这不是偷懒，是这一层能力的真实边界：body 是一个 `Stream`，掐断它只能得到一个截断的
//! 响应，而响应头（包括状态码）早就发出去了——没有任何办法再把它改成 504。
//!
//! 重要的是**这条边界是结构性的，不是一句文档承诺**：包住的表达式就那么一个，想让它覆盖
//! body 得先改变这里的形状。把它含糊地说成"超时覆盖整个请求"，慢流式响应就会
//! 被误以为受保护——那是一条读代码读不出来、只能在事故里发现的差异。
//!
//! 要给流式响应上界，得在 body 那一层做（每块之间自己计时），那是业务的事。
//!
//! # 预算每请求现读
//!
//! `http.handler_timeout` 是**热**字段。这里 `load()` 一次、取一次，不缓存——缓存了就等于
//! 把热字段悄悄降级成"面重启才生效"，而配置文档里写的是热。

use std::time::Duration;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::HttpError;
use crate::state::AppState;

/// 超出预算就回 504 信封。
pub(crate) async fn handler_timeout(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let budget: Duration = state.config.load().config().http.handler_timeout;

    // 在 `request` 被交出去之前把要记的东西取走。**只取 path，不取完整 URI**——
    // query 串里可能有 token 之类的东西，而这条日志是必然会被记下来的。
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    match tokio::time::timeout(budget, next.run(request)).await {
        Ok(response) => response,
        Err(_elapsed) => {
            // 响应里只有一句固定的话，超时的是谁只能在这里说。
            tracing::warn!(
                name: "handler_timed_out",
                target: "http",
                %method,
                path,
                budget_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
                "a handler overran its timeout budget"
            );
            HttpError::timeout().into_response()
        }
    }
}
