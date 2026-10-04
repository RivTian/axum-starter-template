//! The request id: the caller's `x-request-id` when it is safe to repeat, otherwise a new
//! `UUIDv7`; the response carries it back, and the request span and problem documents show it.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// The header that carries the request id.
pub(crate) const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The id of the current request, in the request's extensions.
#[derive(Clone, Debug)]
pub(crate) struct RequestId(pub(crate) String);

/// Keeps the caller's `x-request-id` when it is 1 to 128 of `A-Z a-z 0-9 . _ -`, and
/// otherwise makes a `UUIDv7`; the response carries it back.
pub(crate) async fn request_id(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(&REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|value| is_valid_request_id(value))
        .map_or_else(|| Uuid::now_v7().to_string(), ToString::to_string);
    request.extensions_mut().insert(RequestId(id.clone()));
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID, value);
    }
    response
}

fn is_valid_request_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::is_valid_request_id;

    #[test]
    fn request_ids_are_kept_only_when_they_are_safe() {
        assert!(is_valid_request_id("abc-123_x.y"));
        assert!(is_valid_request_id(&"a".repeat(128)));
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id(&"a".repeat(129)));
        assert!(!is_valid_request_id("has space"));
        assert!(!is_valid_request_id("semi;colon"));
    }
}
