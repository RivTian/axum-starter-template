//! HTTP error responses.
//!
//! [`HttpError`] is the single `Err` type of every handler and extractor in
//! this crate. It carries no message of its own: the variant *is* the meaning,
//! and the status/description table below is the only place either is decided.
//!
//! | Variant | Status | Description on the wire |
//! |---|---|---|
//! | [`HttpError::NotFound`] | 404 | `route not found` |
//! | [`HttpError::MethodNotAllowed`] | 405 | `method not allowed` |
//! | [`HttpError::NotReady`] | 503 | `service is not ready` |
//! | [`HttpError::Timeout`] | 503 | `request processing deadline exceeded` |
//! | [`HttpError::Rejected`] | the framework's own 4xx | that status's canonical reason |
//! | [`HttpError::Internal`] | 500 | `internal server error` |
//!
//! Every row is pinned by the table test at the bottom of this file, so a
//! variant that gets quietly re-coded fails here rather than in a client's
//! retry policy.
//!
//! Descriptions are `&'static str` by construction -- there is no seam to
//! format one through, which is what keeps submitted values, driver text and
//! configuration out of a response body. A business route that must name a
//! resource adds a variant carrying its own `String` and renders it for 4xx
//! only; 5xx never gets one.

use crate::response::GenericResponse;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::any::Any;

#[derive(Debug)]
pub(crate) enum HttpError {
    /// No route matched -> 404. Deliberately does not echo the method or path
    /// back: an unauthenticated caller learns only that nothing matched.
    NotFound,
    /// The path matched but the method did not -> 405. axum still attaches the
    /// `Allow` header, which is the part a client acts on.
    MethodNotAllowed,
    /// A dependency is not usable right now -> 503.
    ///
    /// The line against 500 is "would coming back later help": 503 says the
    /// dependency is down at this moment and the request itself is fine, so the
    /// caller can back off and retry; 500 says this path does not work and
    /// retrying changes nothing. Mixing the two leaves a client's retry policy
    /// with nothing to stand on.
    NotReady,
    /// The application's own request deadline elapsed -> 503, by the same rule
    /// as [`HttpError::NotReady`]: the work did not fit in the budget this
    /// time, not that it can never fit.
    Timeout,
    /// An extractor rejection the framework already classified: keep its
    /// status and that status's canonical reason, never its body or source
    /// chain -- rejection text can contain submitted values or internal type
    /// names.
    ///
    /// Construct through [`HttpError::rejected`], which routes server-error
    /// statuses to [`HttpError::Internal`]. A 5xx must not reach this variant.
    Rejected(StatusCode),
    /// Nothing the caller can act on -> 500 with a fixed description. The cause
    /// goes to the log; the body says only that the server failed.
    Internal,
}

impl HttpError {
    /// Map an extractor rejection. Server-error rejections lose the framework's
    /// status text entirely and are logged instead, so the body stays fixed.
    pub(crate) fn rejected(status: StatusCode, kind: &'static str) -> Self {
        if status.is_server_error() {
            tracing::error!(
                rejection = kind,
                status = status.as_u16(),
                "internal extractor error"
            );
            Self::Internal
        } else {
            Self::Rejected(status)
        }
    }

    /// The one status/description table. Everything else in this module reads
    /// it; nothing else writes it.
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "route not found"),
            Self::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
            Self::NotReady => (StatusCode::SERVICE_UNAVAILABLE, "service is not ready"),
            Self::Timeout => (
                StatusCode::SERVICE_UNAVAILABLE,
                "request processing deadline exceeded",
            ),
            Self::Rejected(status) => (
                *status,
                status.canonical_reason().unwrap_or("invalid request"),
            ),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal server error"),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        // Errors always take the envelope, and `code` is read off the same
        // status the response is actually sent with -- the two cannot drift.
        let (status, description) = self.parts();
        let body = GenericResponse::error(status.as_u16(), description);
        (status, Json(body)).into_response()
    }
}

/// Answer a caught handler panic from the table above, as [`HttpError::Internal`]:
/// an unwind carries no status, so that is the only honest reading of it.
/// Installed once, at the router assembly point in [`crate::finish`].
///
/// The payload is dropped rather than rendered or logged, for two separate
/// reasons. It must not reach the body because `panic!("{input}")` produces a
/// `String` payload -- exactly the class of text 5xx descriptions exist to
/// exclude. It need not reach the log because the default panic hook has
/// already written the message and its location to stderr, which the template
/// expects the log collector to capture. What is recorded here is the one
/// thing stderr cannot carry: the event is emitted inside the request span, so
/// the panic is attributed to a `matched_route` rather than to a thread.
pub(crate) fn panic_response(_payload: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!("handler panicked");
    HttpError::Internal.into_response()
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

    /// One row per variant. The mapping is centralised precisely so it can be
    /// pinned in one place; the point of the table is that adding a variant
    /// without a row here is a visible omission rather than a silent one.
    #[tokio::test]
    async fn every_variant_maps_to_its_status_code() {
        let cases = [
            (
                HttpError::NotFound,
                StatusCode::NOT_FOUND,
                "route not found",
            ),
            (
                HttpError::MethodNotAllowed,
                StatusCode::METHOD_NOT_ALLOWED,
                "method not allowed",
            ),
            (
                HttpError::NotReady,
                StatusCode::SERVICE_UNAVAILABLE,
                "service is not ready",
            ),
            (
                HttpError::Timeout,
                StatusCode::SERVICE_UNAVAILABLE,
                "request processing deadline exceeded",
            ),
            (
                HttpError::Rejected(StatusCode::UNSUPPORTED_MEDIA_TYPE),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Unsupported Media Type",
            ),
            (
                HttpError::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error",
            ),
        ];

        for (error, want_status, want_description) in cases {
            let label = format!("{error:?}");
            let response = error.into_response();
            assert_eq!(response.status(), want_status, "{label}");

            // Errors are envelope-shaped without exception, and `code` always
            // repeats the real status.
            let body = body_json(response).await;
            assert_eq!(body["status"], "error", "{label}");
            assert_eq!(body["code"], u16::from(want_status), "{label}");
            assert_eq!(body["description"], want_description, "{label}");
            assert_eq!(
                body.as_object().expect("an envelope is an object").len(),
                3,
                "{label}: the envelope carries exactly three keys"
            );
        }
    }

    /// The 5xx half of [`HttpError::rejected`]: a rejection the framework calls
    /// a server error keeps neither its status text nor its reason, because
    /// both can quote what was submitted.
    #[tokio::test]
    async fn server_error_rejections_collapse_to_the_fixed_internal_body() {
        let response =
            HttpError::rejected(StatusCode::INTERNAL_SERVER_ERROR, "path").into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body_json(response).await["description"],
            "internal server error"
        );

        // The 4xx half, for contrast: the framework's own classification stands.
        let response = HttpError::rejected(StatusCode::BAD_REQUEST, "query").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["description"], "Bad Request");
    }

    /// [`panic_response`] is an entry point into the table, not a seventh row:
    /// what it answers must stay identical to the `Internal` row rather than
    /// drift into its own wording. Asserting against that row instead of a
    /// literal is what makes the two impossible to separate.
    ///
    /// A `String` payload is the case worth pinning -- `panic!("{input}")`
    /// builds one out of whatever the handler was holding.
    #[tokio::test]
    async fn a_panic_payload_is_answered_from_the_internal_row_and_never_rendered() {
        let response = panic_response(Box::new(String::from("PRIVATE_TOKEN")));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!format!("{:?}", response.headers()).contains("PRIVATE_TOKEN"));
        assert_eq!(
            body_json(response).await,
            body_json(HttpError::Internal.into_response()).await
        );
    }
}
