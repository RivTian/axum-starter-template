//! The probes: `/livez` answers whether the process can take requests at all, `/readyz`
//! whether it should get traffic now. Neither is a problem document or an error log.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Map, Value, json};
use svc_runtime::prelude::*;

use crate::state::AppState;

/// `/livez`: 200 whenever a connection is accepted.
pub(crate) async fn livez() -> Json<Value> {
    Json(json!({ "status": "live" }))
}

/// `/readyz`: 200 while running with every check healthy, 503 otherwise; the body names
/// the phase and whether each check is healthy, never why one is not.
pub(crate) async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let ready = state.readiness.is_ready();
    let checks: Map<String, Value> = state
        .readiness
        .health()
        .checks
        .into_iter()
        .map(|(name, health)| {
            let health = match health {
                Health::Healthy => "healthy",
                Health::Unhealthy(_) => "unhealthy",
            };
            (name.to_string(), Value::from(health))
        })
        .collect();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = json!({
        "status": if ready { "ready" } else { "not ready" },
        "phase": state.readiness.phase().as_str(),
        "checks": checks,
    });
    (status, Json(body))
}
