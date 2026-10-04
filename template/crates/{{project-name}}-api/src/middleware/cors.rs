//! Cross-origin requests from browsers, for the origins in `server.cors_origins`.

use std::time::Duration;

use axum::http::header::{CONTENT_TYPE, LOCATION, RETRY_AFTER};
use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use super::request_id::REQUEST_ID;

/// The CORS layer for these origins, or `None` when the list is empty. The settings have
/// checked every origin; `*` alone allows any.
pub(crate) fn cors(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    let allowed = if origins.iter().any(|origin| origin == "*") {
        AllowOrigin::from(Any)
    } else {
        AllowOrigin::list(
            origins
                .iter()
                .filter_map(|origin| HeaderValue::from_str(origin).ok()),
        )
    };
    Some(
        CorsLayer::new()
            .allow_origin(allowed)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([CONTENT_TYPE, REQUEST_ID])
            .expose_headers([REQUEST_ID, LOCATION, RETRY_AFTER])
            .max_age(Duration::from_secs(600)),
    )
}
