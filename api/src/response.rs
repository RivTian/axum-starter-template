//! HTTP response types.
//!
//! Two established success contracts. Consumers depend on both, so neither is
//! something to "correct" in a refactor; the unit tests below pin them:
//!
//! - [`ApiResponse::Ok`]   -- success with no business data -> the
//!   `{status, code, description}` envelope;
//! - [`ApiResponse::Data`] -- success carrying business data -> **bare JSON**,
//!   with no envelope around it.
//!
//! Error responses are owned by [`crate::error::HttpError`] and are not handled
//! in this module.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The body shared by "success without data" and every error the application
/// can still answer.
///
/// ```json
/// // success
/// { "status": "success", "code": 200, "description": "" }
///
/// // failure
/// { "status": "error", "code": 503, "description": "service is not ready" }
/// ```
///
/// `description` is a `String` so an error can name what went wrong. That is a
/// seam for 4xx only: `HttpError` holds a `&'static str`, which is what keeps
/// submitted values, driver text and configuration out of 5xx bodies. Anything
/// calling [`GenericResponse::error`] directly gives up that guarantee.
#[derive(Debug, Serialize)]
pub(crate) struct GenericResponse {
    pub status: &'static str,
    pub code: u16,
    pub description: String,
}

impl GenericResponse {
    /// Success (code = 200). `description` is an empty string and not an absent
    /// key: a client reading it unconditionally should not special-case success.
    pub(crate) fn ok() -> Self {
        Self {
            status: "success",
            code: 200,
            description: String::new(),
        }
    }

    /// Error. `code` must be the real HTTP status the response is sent with;
    /// nothing here cross-checks the two.
    pub(crate) fn error(code: u16, description: impl Into<String>) -> Self {
        Self {
            status: "error",
            code,
            description: description.into(),
        }
    }
}

/// The success half of the API, as an axum [`IntoResponse`].
///
/// `T` is serialized only by [`ApiResponse::Data`]; a route with no payload
/// writes `ApiResponse::<()>::Ok`, and the turbofish is the point -- "this
/// route returns nothing" stays visible where the route is declared.
///
/// ```rust,ignore
/// // Pattern A: success, no data.
/// async fn ready(State(state): State<AppState>) -> Result<ApiResponse<()>, HttpError> {
///     probe(&state).await?;
///     Ok(ApiResponse::Ok)
/// }
///
/// // Pattern B: success, carrying data.
/// async fn info(State(state): State<AppState>) -> ApiResponse<Info> {
///     ApiResponse::Data(Info::from(state.build))
/// }
/// ```
#[derive(Debug)]
pub(crate) enum ApiResponse<T: Serialize> {
    /// Success: HTTP 200 + `GenericResponse { status: "success" }`.
    Ok,
    /// Success: HTTP 200 + business data `T`, serialized bare with no wrapper.
    Data(T),
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        match self {
            ApiResponse::Ok => (StatusCode::OK, Json(GenericResponse::ok())).into_response(),
            ApiResponse::Data(data) => (StatusCode::OK, Json(data)).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response body");
        serde_json::from_slice(&bytes).expect("the body must be JSON")
    }

    /// Contract: success with data is the business object serialized bare --
    /// the body *is* the object. A renamed multi-field payload is deliberate:
    /// it also shows serde attributes reach the wire unwrapped.
    #[tokio::test]
    async fn data_is_bare_json_without_envelope_keys() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload {
            item_id: &'static str,
            enabled: bool,
        }

        let response = ApiResponse::Data(Payload {
            item_id: "a1",
            enabled: true,
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_json(response).await;
        assert_eq!(body["itemId"], "a1");
        assert_eq!(body["enabled"], true);
        let object = body.as_object().expect("data responds with a JSON object");
        for key in ["status", "code", "description"] {
            assert!(!object.contains_key(key), "bare JSON must not carry {key}");
        }
    }

    /// Contract: success without data is the envelope, all three keys present,
    /// `description` an empty string rather than missing.
    #[tokio::test]
    async fn ok_is_success_envelope() {
        let response = ApiResponse::<()>::Ok.into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_json(response).await;
        assert_eq!(body["status"], "success");
        assert_eq!(body["code"], 200);
        assert_eq!(body["description"], "");
        assert_eq!(
            body.as_object().expect("an envelope is an object").len(),
            3,
            "the envelope carries exactly three keys"
        );
    }
}
