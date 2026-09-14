//! System HTTP API. The caller owns spawning, startup commit and joining.

mod error;
// The sole prelaid exception, explained in architecture §6. Keep the allowance
// here, not on the crate, and exercise all three wrappers in contract tests.
#[allow(dead_code)]
mod extract;
mod handler;
mod response;
mod settings;
mod state;

pub use settings::{HttpSettings, HttpSettingsError};
pub use state::AppState;

use axum::Router;
use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use service_core::lifecycle::LifecycleClosed;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;

pub fn build_router(state: AppState, settings: &HttpSettings) -> Router {
    finish(
        Router::new().nest("/v1", handler::system::routes(settings)),
        state,
        settings,
    )
}

fn finish(router: Router<AppState>, state: AppState, settings: &HttpSettings) -> Router {
    let budget = settings.request_timeout;
    router
        .fallback(|| async { error::HttpError::new(StatusCode::NOT_FOUND, "route not found") })
        .method_not_allowed_fallback(|| async { error::HttpError::new(StatusCode::METHOD_NOT_ALLOWED, "method not allowed") })
        .layer(middleware::from_fn(move |request: Request, next: Next| async move {
            match tokio::time::timeout(budget, next.run(request)).await {
                Ok(response) => response,
                Err(_) => error::HttpError::timeout().into_response(),
            }
        }))
        .layer(TraceLayer::new_for_http()
            .make_span_with(|request: &Request| {
                let route = request.extensions().get::<MatchedPath>().map_or("<unmatched>", MatchedPath::as_str);
                tracing::info_span!("http_request", method = %request.method(), matched_route = route)
            })
            .on_request(())
            .on_response(|response: &Response, latency: std::time::Duration, span: &tracing::Span| {
                tracing::info!(parent: span, status = response.status().as_u16(), latency_ms = latency.as_millis(), "request completed");
            })
            .on_failure(()))
        .with_state(state)
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("HTTP listener bind failed")]
    Bind(#[source] std::io::Error),
    #[error("HTTP startup receiver closed")]
    StartupAbandoned,
    #[error(transparent)]
    Lifecycle(#[from] LifecycleClosed),
    #[error("HTTP transport failed")]
    Transport(#[source] std::io::Error),
}

pub async fn run(
    state: AppState,
    settings: HttpSettings,
    shutdown: CancellationToken,
    started: oneshot::Sender<std::net::SocketAddr>,
) -> Result<(), ServeError> {
    if shutdown.is_cancelled() {
        return Ok(());
    }
    let mut gate = state.lifecycle.clone();
    let router = build_router(state, &settings);
    let listener = TcpListener::bind(settings.listen)
        .await
        .map_err(ServeError::Bind)?;
    let address = listener.local_addr().map_err(ServeError::Bind)?;
    if started.send(address).is_err() {
        return if shutdown.is_cancelled() {
            Ok(())
        } else {
            Err(ServeError::StartupAbandoned)
        };
    }
    if !gate.wait_running(&shutdown).await? {
        return Ok(());
    }
    serve(listener, router, shutdown)
        .await
        .map_err(ServeError::Transport)
}

// Normal completion means graceful drain. Aborting this future is not proof
// that axum's library-owned connections were joined (see retained M1 tests).
async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod tests;
