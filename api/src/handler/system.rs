use crate::error::HttpError;
use crate::response::Envelope;
use crate::{AppState, HttpSettings};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use service_core::lifecycle::Phase;

pub(crate) fn routes(settings: &HttpSettings) -> Router<AppState> {
    let probe = settings.ready_probe_timeout;
    Router::new()
        .route("/service/health", get(|| async { Envelope::success() }))
        .route("/service/info", get(info))
        .route(
            "/service/ready",
            get(move |State(state): State<AppState>| async move {
                if state.lifecycle.phase() != Some(Phase::Running) {
                    return Err(HttpError::unavailable());
                }
                match tokio::time::timeout(probe, state.storage.health()).await {
                    Ok(Ok(())) if state.lifecycle.phase() == Some(Phase::Running) => {
                        Ok(Envelope::success())
                    }
                    _ => Err(HttpError::unavailable()),
                }
            }),
        )
}

#[derive(Serialize)]
struct Info {
    service: &'static str,
    version: &'static str,
}
async fn info(State(state): State<AppState>) -> Json<Info> {
    Json(Info {
        service: state.build.service,
        version: state.build.version,
    })
}
