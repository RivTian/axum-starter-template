//! The routes and the middleware stack; the middleware module explains the order.

use axum::Router;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use svc_util::prelude::*;

use crate::problem::{self, ApiError};
use crate::settings::ServerSettings;
use crate::state::AppState;
use crate::{middleware, probes, todo};

/// The router of the service, with its middleware, over the given state and settings.
pub fn router(state: AppState, settings: &ServerSettings) -> Router {
    let problem_state = (state.clone(), settings.request_timeout);
    Router::new()
        .route("/livez", get(probes::livez))
        .route("/readyz", get(probes::readyz))
        .route("/v1/todos", get(todo::list).post(todo::create))
        .route("/v1/todos/{id}", get(todo::get).delete(todo::delete))
        .route("/v1/todos/{id}/complete", post(todo::complete))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::body_limit(settings.body_limit_bytes))
        .layer(middleware::catch_panic())
        .layer(from_fn_with_state(problem_state, problem::render))
        .layer(from_fn(middleware::access_log))
        .layer(from_fn(middleware::request_id))
        .with_state(state)
}

/// A path without a route: 404.
async fn not_found() -> ApiError {
    ApiError(Error::new_down(ErrorType::HTTPStatus(404)))
}

/// A known path without this method: 405; the router adds `Allow`.
async fn method_not_allowed() -> ApiError {
    ApiError(Error::new_down(ErrorType::HTTPStatus(405)))
}
