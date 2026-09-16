//! 包装提取器：把 axum 的 rejection 换成统一信封。
//!
//! # 它们做的事**只有一件**
//!
//! 解析仍然由 axum 完成，状态码也直接取 axum 给的那一个。这三个包装唯一改变的是
//! **失败时的响应体**：axum 回 `text/plain`（"Failed to parse the request body as JSON…"），
//! 这里回统一的三键信封。
//!
//! 不重新编一张状态码表是刻意的，理由写在 [`HttpError::from_rejection`] 上——一句话：
//! axum 的分档比手抄表细，而手抄表一定会漂。
//!
//! # 为什么值得为这一件事写三个类型
//!
//! 因为不这样做的话，「所有错误响应长一个样」就只剩一条文档纪律，而写 handler 的人打
//! `Json` 时补全推给他的第一个候选就是 axum 的那个。所以这条禁令交给
//! `clippy.toml` 的 `disallowed-types` 执行——工具比评审可靠。
//!
//! # `Json` 两边都能用
//!
//! [`Json`] 既是提取器（`FromRequest`）也是响应类型（`IntoResponse`），和 `axum::Json`
//! 一样。这样使用者在任何位置都不需要念 axum 的那个名字，禁令不会把人逼进死角。

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::HttpError;
use crate::response;

/// JSON 请求体 / JSON 响应体。
///
/// 失败时的状态码沿用 axum：语法错误 400、缺 `Content-Type` 415、语义不符 422、
/// 超过 `DefaultBodyLimit` 413。四档在 `tests/contract.rs` 里各有一条用例，并断言
/// 它们**彼此不相等**（不许拍平成一个 400）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Json<T>(pub T);

/// 路径参数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Path<T>(pub T);

/// 查询串。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = HttpError;

    // 这三处 `allow` 是 `disallowed-types` 在全仓库仅有的例外：包装器必须能念出被包装的
    // 那个名字。打在函数上而不是模块或 crate 上——`allow` 只打在最小单元上。
    #[allow(clippy::disallowed_types)]
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Json::<T>::from_request(request, state).await {
            Ok(axum::extract::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(HttpError::from_rejection(
                rejection.status(),
                rejection.body_text(),
            )),
        }
    }
}

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = HttpError;

    #[allow(clippy::disallowed_types)]
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(HttpError::from_rejection(
                rejection.status(),
                rejection.body_text(),
            )),
        }
    }
}

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = HttpError;

    #[allow(clippy::disallowed_types)]
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(HttpError::from_rejection(
                rejection.status(),
                rejection.body_text(),
            )),
        }
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        response::json(StatusCode::OK, &self.0)
    }
}
