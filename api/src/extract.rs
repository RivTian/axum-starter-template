//! 提取器：把 axum 的拒绝也收进统一信封
//!
//! `lib.rs` 的第三条契约说「所有错误 → 同形信封」。直接用 `axum::Json` 会让这句话
//! 不成立：拒绝发生在 handler **被调用之前**，body 是一行 `text/plain`。客户端拿它去
//! `JSON.parse`，报的是语法错误——而真正的原因是「你少了个逗号」。
//!
//! 本模块把三个提取器各包一层，只换信封，另外两样原样留：
//!
//! - **状态码取 `rejection.status()`**，不一律拍成 400。axum 已经分好了：体不是 JSON
//!   → 400，是 JSON 但形状对不上 → 422，`Content-Type` 不对 → 415，体过大 → 413。
//!   拍平会让调用方分不出「格式发错了」与「少了个字段」，而这两件事的修法不同。
//! - **文案取 `rejection.body_text()`**：那就是 axum 本来要写进 body 的那段字，用它
//!   意味着包一层只换外壳、不换诊断。
//!
//! 提取器也会报 5xx（`Path` 取的参数个数与路由对不上等），那是本服务的 bug 不是调用方
//! 的，由 [`HttpError`] 按内部错误统一处理：原文进日志，对外泛化。
//!
//! **加 handler 一律用这里的 `Json` / `Query` / `Path`**，不要用 `axum::` 下的同名类型。

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::error::HttpError;

/// `axum::Json` 的替身：拒绝走信封。
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let axum::Json(value) = axum::Json::<T>::from_request(request, state)
            .await
            .map_err(|rejection| HttpError::Rejected(rejection.status(), rejection.body_text()))?;
        Ok(Self(value))
    }
}

/// `axum::extract::Query` 的替身：拒绝走信封。
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let axum::extract::Query(value) =
            axum::extract::Query::<T>::from_request_parts(parts, state)
                .await
                .map_err(|rejection| {
                    HttpError::Rejected(rejection.status(), rejection.body_text())
                })?;
        Ok(Self(value))
    }
}

/// `axum::extract::Path` 的替身：拒绝走信封。
///
/// 路径参数写错的概率不如请求体高，但漏包一样破契约：一个把 id 写成 `abc` 的请求，
/// 拿到的是一行 `text/plain`，而同一个调用方在同一次会话里拿到的其他错误全是信封。
pub struct Path<T>(pub T);

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let axum::extract::Path(value) = axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| HttpError::Rejected(rejection.status(), rejection.body_text()))?;
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request as HttpRequest, StatusCode, header};
    use axum::routing::{get, post};
    use serde::Deserialize;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    #[derive(Deserialize)]
    struct Payload {
        name: String,
    }

    async fn echo(Json(payload): Json<Payload>) -> String {
        payload.name
    }

    /// 打一次 `POST /echo`，回 (状态码, 响应体)；响应体解析失败即判定「不是信封」
    async fn post_echo(content_type: &str, body: &'static str) -> (StatusCode, Value) {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();
        let response = Router::new()
            .route("/echo", post(echo))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).expect("拒绝也必须是 JSON 信封");
        (status, json)
    }

    /// 三种拒绝各自保住 axum 判的状态码，且都是信封——拍平成 400 就分不出它们
    #[tokio::test]
    async fn rejections_keep_their_status_and_arrive_as_envelopes() {
        let cases = [
            (r#"{"name":"#, StatusCode::BAD_REQUEST, "application/json"),
            (
                r#"{"nome":"x"}"#,
                StatusCode::UNPROCESSABLE_ENTITY,
                "application/json",
            ),
            (
                r#"{"name":"x"}"#,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "text/plain",
            ),
        ];
        for (body, expected, content_type) in cases {
            let (status, json) = post_echo(content_type, body).await;
            assert_eq!(status, expected, "{body}");
            assert_eq!(json["status"], "error");
            assert_eq!(json["code"], expected.as_u16());
            // 诊断没被吞掉：这里是 axum 本来要写进 body 的那段字
            let description = json["description"].as_str().unwrap();
            assert!(
                !description.is_empty() && description != "internal error",
                "{json}"
            );
        }
    }

    /// 合法请求照常进 handler：包装只在失败路径上改行为
    #[tokio::test]
    async fn a_valid_body_still_reaches_the_handler() {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"ok"}"#))
            .unwrap();
        let response = Router::new()
            .route("/echo", post(echo))
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"ok");
    }

    /// 路径参数个数与路由对不上是**本服务的 bug**：500 + 泛化文案，原文只进日志
    #[tokio::test]
    async fn a_server_side_rejection_does_not_leak_its_message() {
        async fn two(Path((a, b)): Path<(i64, i64)>) -> String {
            format!("{a}-{b}")
        }

        let request = HttpRequest::builder()
            .uri("/x/1")
            .body(Body::empty())
            .unwrap();
        let response = Router::new()
            .route("/x/{a}", get(two))
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["code"], 500);
        assert_eq!(json["description"], "internal error");
    }

    /// 查询串解析失败：400 信封（`Query` 只有这一种拒绝）
    #[tokio::test]
    async fn a_bad_query_string_is_an_envelope() {
        #[derive(Deserialize)]
        struct Filter {
            limit: u32,
        }

        async fn list(Query(filter): Query<Filter>) -> String {
            filter.limit.to_string()
        }

        let request = HttpRequest::builder()
            .uri("/list?limit=many")
            .body(Body::empty())
            .unwrap();
        let response = Router::new()
            .route("/list", get(list))
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], "error");
        assert_eq!(json["code"], 400);
    }
}
