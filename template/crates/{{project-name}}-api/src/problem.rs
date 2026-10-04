//! The failure side of the HTTP contract. Handlers return [`ApiError`]; the problem
//! middleware renders every failed response, the framework's own included, as an
//! `application/problem+json` document (RFC 9457), and logs the error once.

mod status;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use svc_util::prelude::*;

pub use status::{Outcome, outcome};

use crate::middleware::RequestId;
use crate::response::ApiResponse;
use crate::state::AppState;

/// A failed request: the error, rendered later by the problem middleware.
#[derive(Debug)]
pub struct ApiError(pub BError);

impl From<BError> for ApiError {
    fn from(error: BError) -> Self {
        ApiError(error)
    }
}

/// What a handler returns: a success shape or an error.
pub type ApiResult<T> = std::result::Result<ApiResponse<T>, ApiError>;

/// An error waiting for the problem middleware, which knows the request. Response extensions
/// must be `Clone`, which `BError` is not, so the error is shared.
#[derive(Clone, Debug)]
struct Pending(Arc<Error>);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        pending(self.0)
    }
}

/// An empty 500 carrying the error for the problem middleware.
pub(crate) fn pending(error: BError) -> Response {
    let mut response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
    response.extensions_mut().insert(Pending(Arc::from(error)));
    response
}

/// The problem middleware. It also enforces `server.request_timeout`, so that a request that
/// takes too long is answered like any other failure.
pub(crate) async fn render(
    State((state, timeout)): State<(AppState, Duration)>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let request_id = (request.extensions().get::<RequestId>())
        .map(|id| id.0.clone())
        .unwrap_or_default();
    let mut response = if let Ok(response) = tokio::time::timeout(timeout, next.run(request)).await
    {
        response
    } else {
        let mut error = Error::explain(
            ErrorType::HTTPStatus(503),
            format!("the request took longer than {}s", timeout.as_secs()),
        );
        error.set_retry(true);
        pending(error)
    };
    let error = match response.extensions_mut().remove::<Pending>() {
        Some(Pending(error)) => error,
        None if needs_problem(&response) => {
            let (parts, body) = response.into_parts();
            let error = framework_error(parts.status, body).await;
            response = Response::from_parts(parts, Body::empty());
            Arc::from(error)
        }
        None => return response,
    };
    let outcome = outcome(&error);
    let status = outcome.status.as_u16();
    log_error!(
        outcome.level,
        &*error,
        http.response.status_code = status,
        "request failed"
    );
    let problem = Problem::new(&outcome, &error, path, request_id, state.service_name);
    // A struct of strings and a number always serializes.
    let body = serde_json::to_vec(&problem).unwrap_or_default();
    let mut rendered = Response::new(Body::from(body));
    *rendered.status_mut() = outcome.status;
    *rendered.headers_mut() = response.headers().clone();
    let headers = rendered.headers_mut();
    headers.remove(CONTENT_LENGTH);
    let json = HeaderValue::from_static("application/problem+json");
    headers.insert(CONTENT_TYPE, json);
    if outcome.retry_after {
        headers.insert(RETRY_AFTER, HeaderValue::from_static("1"));
    }
    rendered
}

/// A failed response of the framework: without a body of its own yet, or the plain text of
/// an extractor's rejection, such as axum's `Query` gives. JSON answers, such as `/readyz`
/// gives, are left as they are.
fn needs_problem(response: &Response) -> bool {
    let status = response.status();
    let plain = response
        .headers()
        .get(CONTENT_TYPE)
        .is_none_or(|value| (value.to_str()).is_ok_and(|value| value.starts_with("text/plain")));
    (status.is_client_error() || status.is_server_error()) && plain
}

/// The error behind a failed response of the framework; its text, such as why a query
/// string did not parse, is the context. A 5xx is this service's own mistake.
async fn framework_error(status: StatusCode, body: Body) -> BError {
    let etype = ErrorType::HTTPStatus(status.as_u16());
    // The framework's texts are short; a longer body is not one of them and is left out.
    let text = (axum::body::to_bytes(body, 4096).await.ok())
        .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
        .filter(|text| !text.trim().is_empty());
    let error = match text {
        Some(text) => Error::explain(etype, text),
        None => Error::new(etype),
    };
    if status.is_server_error() {
        error.into_in()
    } else {
        error.into_down()
    }
}

/// A problem document. `type` is a URN for client errors of an error kind and `about:blank`
/// otherwise; `title` is the kind's title, or the status reason when it has none; `detail`
/// appears only when the outcome exposes it and the error has a context.
#[derive(Debug, Serialize)]
struct Problem {
    #[serde(rename = "type")]
    kind: String,
    title: String,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    instance: String,
    request_id: String,
}

impl Problem {
    fn new(
        outcome: &Outcome,
        error: &Error,
        path: String,
        request_id: String,
        service: &str,
    ) -> Self {
        let status = outcome.status;
        let (kind, title) = match error.etype() {
            ErrorType::Kind(kind) if status.is_client_error() => (
                format!("urn:{service}:problem:{}", kebab(kind.name())),
                kind.title(),
            ),
            _ => ("about:blank".to_string(), None),
        };
        let reason = || status.canonical_reason().unwrap_or("Error");
        let title = title.unwrap_or_else(reason).to_string();
        Problem {
            kind,
            title,
            status: status.as_u16(),
            detail: (error.context.as_ref())
                .filter(|_| outcome.expose)
                .map(|context| context.as_str().to_string()),
            instance: path,
            request_id,
        }
    }
}

/// `TodoNotFound` as `todo-not-found`; a run of capitals is one word, as in `HTTPStatus` to
/// `http-status`.
fn kebab(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (index, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && index > 0 {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if previous.is_ascii_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_ascii_uppercase() && next_is_lower)
            {
                out.push('-');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests;
