//! handler panic 的响应。
//!
//! # 为什么不能用 `CatchPanicLayer::new()`
//!
//! 它的默认响应（tower-http 0.7.1 的 `DefaultResponseForPanic`）是：
//!
//! ```text
//! 500 Internal Server Error
//! content-type: text/plain; charset=utf-8
//!
//! Service panicked
//! ```
//!
//! 状态码是对的，body 不是信封。这与超时那条（[`super::timeout`]）是**同一类**缺陷——
//! 第三方层自带一套"出错时回什么"，而它不知道本服务有信封。所以这里用
//! `CatchPanicLayer::custom`，把响应换成我们自己的。
//!
//! # 为什么捕获 panic 而不是让进程死
//!
//! release 剖面选的是 `panic = "unwind"`。一个业务 handler 里的 `unwrap()` 不应该把整个进程（连同
//! 另外几个正常的任务面、正在优雅关停的连接）一起带走。捕获之后，这次请求 500，其余照常。
//!
//! 注意这**不覆盖** `spawn` 出去的任务：那些 panic 由 `TaskSupervisor` 收成
//! `ExitKind::Panicked`。两条路径各管一段，没有重叠。
//!
//! # panic 负载只进日志
//!
//! `payload` 里通常是 `assert!` 的那句话，可能带上任何局部变量的值。响应里给的是一句固定
//! 的话，负载进日志——本层「凡是响应里不能说的，日志里必须说」的第三个落点。

use std::any::Any;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::error::HttpError;

/// `CatchPanicLayer::custom` 的响应工厂。
///
/// 写成一个普通的 `fn` 项而不是闭包：`ResponseForPanic` 对
/// `FnMut(Box<dyn Any + Send + 'static>) -> Response<B> + Clone` 有一条覆盖实现，
/// 而 `fn` 项天生 `Copy`，于是不需要为了满足 `Clone` 再包一层。
pub(crate) fn respond_to_panic(payload: Box<dyn Any + Send + 'static>) -> Response {
    let detail = describe(payload.as_ref());

    tracing::error!(
        name: "handler_panicked",
        target: "http",
        panic = detail,
        "a handler panicked and was turned into a 500"
    );

    HttpError::server(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "the request could not be processed",
    )
    .into_response()
}

/// 把 panic 负载还原成一句话。
///
/// `panic!("...")` 的负载是 `&'static str`，`panic!("{x}")` 的是 `String`，
/// 用 `panic_any` 抛别的类型时两样都不是——最后那种只能给个占位符，没有别的办法：
/// `dyn Any` 上没有 `Display`。
fn describe(payload: &(dyn Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        return text;
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.as_str();
    }
    "<non-string panic payload>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_payload_shapes_are_all_described() {
        assert_eq!(describe(&"boom"), "boom");
        assert_eq!(describe(&"boom".to_owned()), "boom");
        assert_eq!(describe(&42_u8), "<non-string panic payload>");
    }

    #[test]
    fn the_response_is_a_500_and_does_not_leak_the_payload() {
        let response = respond_to_panic(Box::new("secret value was 42"));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        // body 是信封，里面只有那句固定的话；负载在日志里。
        // 完整的 body 断言在 `tests/contract.rs`，这里只钉状态码与 content-type。
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/json"
        );
    }
}
