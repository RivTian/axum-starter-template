//! HTTP 通用响应类型
//!
//! 两条成功响应契约，与 [`crate::error::HttpError`] 的错误信封合起来就是这个面
//! 对外的全部形态：
//!
//! - [`ApiResponse::Ok`]   — 成功、无业务数据 → `{status, code, description}` 信封
//! - [`ApiResponse::Data`] — 成功、携带业务数据 → **裸 JSON**（不套信封）
//!
//! 有数据就裸给，而不是一律套一层 `{data: ...}`：多一层包装，每个消费者都要多剥
//! 一次，而成功路径上那个恒为 200 的 `code` 不带任何信息——状态码已经说过一遍了。
//! 无数据时反过来：裸给等于回一个空 body，客户端分不清「成功但没东西」与「响应被
//! 中途截断」，这时信封才有意义。
//!
//! 错误响应一律由 [`crate::error::HttpError`] 负责，不在本模块处理：状态码与信封的
//! 对应关系只写在那一处。两边共用下面的 [`GenericResponse`]，成功信封与错误信封
//! 因此不可能长歪成两种形状。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// 「成功无数据」与「所有错误」共用的通用响应体。
///
/// ```json
/// // 成功
/// { "status": "success", "code": 200, "description": "" }
///
/// // 失败
/// { "status": "error", "code": 404, "description": "no route for GET /v1/x" }
/// ```
#[derive(Debug, Serialize)]
pub struct GenericResponse {
    pub status: &'static str,
    pub code: u16,
    pub description: String,
}

impl GenericResponse {
    /// 成功信封（`code` 恒为 200：本枚举的成功分支只有 200 这一种）
    pub fn ok() -> Self {
        Self {
            status: "success",
            code: 200,
            description: String::new(),
        }
    }

    /// 错误信封；`code` 由调用方传入的 HTTP 状态码填，两处不会不一致
    pub fn error(code: u16, description: impl Into<String>) -> Self {
        Self {
            status: "error",
            code,
            description: description.into(),
        }
    }
}

/// 统一的成功响应，实现 axum 的 [`IntoResponse`]。
///
/// handler 的返回类型写成 `Result<ApiResponse<T>, HttpError>`，两个分支各自对应
/// 上面的一条契约：
///
/// ```rust,ignore
/// // 无数据：回成功信封
/// async fn touch(...) -> Result<ApiResponse<()>, HttpError> {
///     service.touch().await?;
///     Ok(ApiResponse::Ok)
/// }
///
/// // 有数据：回裸 JSON
/// async fn detail(...) -> Result<ApiResponse<Detail>, HttpError> {
///     Ok(ApiResponse::Data(service.detail().await?))
/// }
/// ```
#[derive(Debug)]
pub enum ApiResponse<T: Serialize> {
    /// 成功，200 + 成功信封
    Ok,
    /// 成功，200 + 业务数据 `T`（裸序列化，无外层包装）
    Data(T),
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        match self {
            Self::Ok => (StatusCode::OK, Json(GenericResponse::ok())).into_response(),
            Self::Data(data) => (StatusCode::OK, Json(data)).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).expect("响应体应是 JSON")
    }

    /// 契约一：成功带数据 = 业务对象**裸序列化**，响应体就是对象本身。
    /// 判定方式是信封的三个键一个都不在——只断言业务字段在，套了信封也照样过
    #[tokio::test]
    async fn data_is_bare_json_without_envelope_keys() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload {
            item_id: &'static str,
            enabled: bool,
        }

        let payload = Payload {
            item_id: "i1",
            enabled: true,
        };
        let response = ApiResponse::Data(payload).into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["itemId"], "i1");
        assert_eq!(json["enabled"], true);
        let object = json.as_object().unwrap();
        for key in ["status", "code", "description"] {
            assert!(!object.contains_key(key), "裸 JSON 不得带 {key}");
        }
    }

    /// 契约二：成功无数据 = 成功信封，恰好三键，`description` 是空串而不是缺失
    #[tokio::test]
    async fn ok_is_a_success_envelope() {
        let response = ApiResponse::<()>::Ok.into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["status"], "success");
        assert_eq!(json["code"], 200);
        assert_eq!(json["description"], "");
        assert_eq!(json.as_object().unwrap().len(), 3, "信封恰好三键");
    }

    /// 成功信封与错误信封同形：键名与顺序都由同一个结构体决定
    #[tokio::test]
    async fn both_envelopes_share_one_shape() {
        let ok = body_json(ApiResponse::<()>::Ok.into_response()).await;
        let error = serde_json::to_value(GenericResponse::error(404, "x")).unwrap();

        let keys = |value: &serde_json::Value| {
            value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&ok), keys(&error));
        assert_eq!(error["status"], "error");
        assert_eq!(error["code"], 404);
        assert_eq!(error["description"], "x");
    }
}
