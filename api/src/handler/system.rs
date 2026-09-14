use crate::error::HttpError;
use crate::response::ApiResponse;
use crate::{AppState, HttpSettings};
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;
use service_core::lifecycle::Phase;

pub(crate) fn routes(settings: &HttpSettings) -> Router<AppState> {
    let probe = settings.ready_probe_timeout;
    Router::new()
        .route("/service/health", get(|| async { ApiResponse::<()>::Ok }))
        .route("/service/info", get(info))
        .route(
            "/service/ready",
            get(move |State(state): State<AppState>| async move {
                if state.lifecycle.phase() != Some(Phase::Running) {
                    return Err(HttpError::NotReady);
                }
                match tokio::time::timeout(probe, state.storage.health()).await {
                    Ok(Ok(())) if state.lifecycle.phase() == Some(Phase::Running) => {
                        Ok(ApiResponse::<()>::Ok)
                    }
                    _ => Err(HttpError::NotReady),
                }
            }),
        )
}

#[derive(Serialize)]
struct Info {
    service: &'static str,
    version: &'static str,
}

// `Data` and not the envelope: success carrying a payload is bare JSON. Adding
// fields here is a contract change for every consumer, and deployment detail
// (config or database paths, environment values, runtime topology) stays out.
async fn info(State(state): State<AppState>) -> ApiResponse<Info> {
    ApiResponse::Data(Info {
        service: state.build.service,
        version: state.build.version,
    })
}
