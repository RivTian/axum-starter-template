use crate::response::Envelope;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub(crate) struct HttpError {
    status: StatusCode,
    description: &'static str,
}

impl HttpError {
    pub(crate) fn new(status: StatusCode, description: &'static str) -> Self {
        Self {
            status,
            description,
        }
    }
    pub(crate) fn unavailable() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "service is not ready")
    }
    pub(crate) fn timeout() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "request processing deadline exceeded",
        )
    }
    pub(crate) fn rejected(status: StatusCode, kind: &'static str) -> Self {
        // Rejection text can contain submitted values or internal type names.
        // Keep the framework's status, not its unfiltered body/source chain.
        if status.is_server_error() {
            tracing::error!(
                rejection = kind,
                status = status.as_u16(),
                "internal extractor error"
            );
            Self::new(status, "internal server error")
        } else {
            Self::new(
                status,
                status.canonical_reason().unwrap_or("invalid request"),
            )
        }
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.status, Envelope::error(self.status, self.description)).into_response()
    }
}
