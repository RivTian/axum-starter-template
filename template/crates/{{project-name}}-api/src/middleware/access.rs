//! The request span and the access log.

use std::net::SocketAddr;
use std::time::Instant;

use axum::extract::{ConnectInfo, MatchedPath, Request};
use axum::http::Version;
use axum::http::header::USER_AGENT;
use axum::middleware::Next;
use axum::response::Response;
use tracing::field::Empty;
use tracing::{Instrument, Level, Span};

use super::RequestId;

/// Runs the request in a span named `request`, created at error level so that the span and
/// its `request_id` stay on every log line of the request whatever the filter; when the
/// request ends, also when the client goes away first, logs `request finished` with target
/// `svc_api::access`.
pub(crate) async fn access_log(request: Request, next: Next) -> Response {
    let span = tracing::error_span!(
        "request",
        http.request.method = %request.method(),
        url.path = request.uri().path(),
        http.route = Empty,
        client.address = Empty,
        client.port = Empty,
        network.protocol.version = protocol(request.version()),
        user_agent.original = Empty,
        request_id = Empty,
    );
    if let Some(route) = request.extensions().get::<MatchedPath>() {
        span.record("http.route", route.as_str());
    }
    if let Some(ConnectInfo(client)) = request.extensions().get::<ConnectInfo<SocketAddr>>() {
        span.record("client.address", tracing::field::display(client.ip()));
        span.record("client.port", client.port());
    }
    let agent = request.headers().get(USER_AGENT);
    if let Some(agent) = agent.and_then(|value| value.to_str().ok()) {
        span.record("user_agent.original", agent);
    }
    if let Some(id) = request.extensions().get::<RequestId>() {
        span.record("request_id", id.0.as_str());
    }
    let mut finished = Finished {
        span: span.clone(),
        started: Instant::now(),
        status: None,
    };
    let response = next.run(request).instrument(span).await;
    finished.status = Some(response.status().as_u16());
    response
}

/// Logs the end of a request when dropped: after the response, or when the client went away
/// and the request was cancelled.
struct Finished {
    span: Span,
    started: Instant,
    status: Option<u16>,
}

impl Drop for Finished {
    fn drop(&mut self) {
        let duration = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _entered = self.span.enter();
        if let Some(status) = self.status {
            tracing::event!(
                target: "svc_api::access",
                Level::INFO,
                http.response.status_code = status,
                http.server.request.duration = duration,
                "request finished"
            );
        } else {
            tracing::event!(
                target: "svc_api::access",
                Level::INFO,
                http.server.request.duration = duration,
                cancelled = true,
                "request finished"
            );
        }
    }
}

fn protocol(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "0.9",
        Version::HTTP_10 => "1.0",
        Version::HTTP_2 => "2",
        Version::HTTP_3 => "3",
        _ => "1.1",
    }
}
