//! 响应渲染：一处写 content-type，一处写错误信封。
//!
//! # 信封长什么样
//!
//! ```text
//! { "error": "not_found", "message": "no route matches this path", "status": 404 }
//! ```
//!
//! 三个键各有各的活儿，少一个就会有一类问题变得不可观察：
//!
//! - `error` —— 稳定的短名，snake_case，**不随文案改动**。它是唯一能把状态码相同、
//!   原因不同的几档分开的东西：`/readyz` 的三档（还没起来 / 正在排空 / 存储探测失败）
//!   全是 503，只看状态码它们一模一样。
//! - `message` —— 给人读的一句话。4xx 可以带细节（是哪个字段、期望什么类型），
//!   5xx **只能**是一句 `&'static str`——这条由类型保证，见 [`crate::error`]。
//! - `status` —— 数字状态码。它看起来是冗余的，直到 body 被从 HTTP 帧上摘下来：
//!   日志里的一行、消息队列里的一条、客户端缓存里的一份，都只剩 body。顺带它给契约
//!   测试一条最便宜的回归判据——`body["status"] == 响应的状态码`——于是"只在一个地方
//!   改了状态码"当场就红（比如有人把超时的 504 "修"回 408）。
//!
//! # 键名会不会撞上业务字段
//!
//! 会。`message` 尤其常见。所以键名集中在 [`ENVELOPE_KEYS`] 这**一个**常量里：要改就
//! 改这里，渲染和契约测试同时跟着走，不存在"改了一半"的中间状态。模板自己的成功响应
//! 刻意避开了这三个名字（`live` / `ready` / `service` / `version` / `uptime_seconds`），
//! 所以「成功响应里三个键一个都不在」这条断言测的是真事，不是自我实现的预言。
//!
//! # 为什么不用 `axum::Json` 渲染
//!
//! `axum::Json` 和 `axum::extract::Json` 是**同一个类型**，而后者在 `clippy.toml` 的
//! `disallowed-types` 名单上——用它就要在这里再开一个 `allow`。自己写一层换来两件事：
//! content-type 是我们写死的常量，序列化失败有一条显式的、会留下日志的兜底路径。
//! 使用者写业务 handler 时用 [`crate::Json`]，它两边都能用，不必碰 axum 的那个。

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Serialize;

/// 错误信封的三个键名。**渲染与断言共用这一份。**
pub const ENVELOPE_KEYS: [&str; 3] = ["error", "message", "status"];

/// 内容类型。RFC 8259 规定 JSON 一律 UTF-8，所以不带 `charset` 参数（与 axum 一致）。
const CONTENT_TYPE_JSON: HeaderValue = HeaderValue::from_static("application/json");

/// 连信封都序列化不出来时的最后一层。**手写的字节，不经过 serde。**
///
/// 它的键名与 [`ENVELOPE_KEYS`] 重复了一份——这是故意的：兜底路径不能再依赖任何可能
/// 失败的东西。重复的代价由 `fallback_body_is_a_well_formed_envelope` 这条用例兜着。
const FALLBACK_BODY: &[u8] =
    br#"{"error":"internal","message":"the response could not be serialized","status":500}"#;

/// 信封的线上形状。
#[derive(Debug, Serialize)]
struct Envelope<'a> {
    error: &'a str,
    message: &'a str,
    status: u16,
}

/// 渲染一个错误信封。
pub(crate) fn envelope(status: StatusCode, error: &str, message: &str) -> Response {
    json(
        status,
        &Envelope {
            error,
            message,
            status: status.as_u16(),
        },
    )
}

/// 渲染任意一份 JSON 响应体。
///
/// 序列化失败不会 panic，也不会静默变成空 body：记一条 `error` 事件，返回 500 信封。
/// 这一层的纪律是「凡是响应里不能说的，日志里必须说」——这里响应里说的是一句无信息的
/// 通用句子，真正的原因（哪个类型、哪个字段）只在日志里。
pub(crate) fn json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => raw(status, bytes),
        Err(error) => {
            tracing::error!(
                name: "response_serialization_failed",
                target: "http",
                %error,
                type_name = std::any::type_name::<T>(),
                "a response body could not be serialized"
            );
            raw(StatusCode::INTERNAL_SERVER_ERROR, FALLBACK_BODY.to_vec())
        }
    }
}

fn raw(status: StatusCode, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    drop(
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, CONTENT_TYPE_JSON),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_body_is_a_well_formed_envelope() {
        // 兜底 body 是手写字节，绕过了 serde。这条用例是它与 `ENVELOPE_KEYS` 之间
        // 唯一的联系——没有它，改了键名之后兜底路径会悄悄发出一份旧形状。
        let parsed: serde_json::Value = serde_json::from_slice(FALLBACK_BODY).unwrap();
        let object = parsed.as_object().unwrap();

        assert_eq!(object.len(), ENVELOPE_KEYS.len());
        for key in ENVELOPE_KEYS {
            assert!(object.contains_key(key), "兜底 body 缺少 `{key}`");
        }
        // `status` 与信封里写的数字必须一致，这正是它存在的理由。
        assert_eq!(object["status"], 500);
    }

    #[test]
    fn a_rendered_envelope_echoes_its_own_status() {
        let response = envelope(StatusCode::NOT_FOUND, "not_found", "nope");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }
}
