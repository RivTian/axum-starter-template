//! Problem rendering of the framework's own answers, on a router of its own.

use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Query;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use svc_domain::todo::TodoUseCases;
use svc_runtime::prelude::*;
use svc_test_utils::clock::FakeClock;
use svc_test_utils::publisher::RecordingPublisher;
use svc_test_utils::repo::FakeTodoRepository;
use tower::ServiceExt;

use super::{kebab, render};
use crate::extract::ApiPath;
use crate::state::AppState;

type TestResult = Result<(), Box<dyn StdError>>;

#[derive(Debug, Deserialize)]
struct Limit {
    #[expect(dead_code, reason = "only parsing matters here")]
    limit: u32,
}

/// Routes that answer the way the framework does, behind problem rendering.
fn app() -> Router {
    let todos = TodoUseCases::new(
        Arc::new(FakeTodoRepository::default()),
        Arc::new(RecordingPublisher::default()),
        Arc::new(FakeClock::default()),
    );
    let readiness = Readiness::new(PhaseWatch::new(), HealthRegistry::default());
    let state = AppState::new(Arc::new(todos), readiness, "acme-svc");
    Router::new()
        .route(
            "/query",
            get(|Query(_): Query<Limit>| async { StatusCode::OK }),
        )
        .route(
            "/{a}/{b}",
            get(|ApiPath(_): ApiPath<String>| async { StatusCode::OK }),
        )
        .route(
            "/json",
            get(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "ready": false })),
                )
            }),
        )
        .layer(from_fn_with_state((state, Duration::from_secs(1)), render))
}

async fn get_json(uri: &str) -> Result<(StatusCode, Option<String>, Value), Box<dyn StdError>> {
    let response = app()
        .oneshot(Request::get(uri).body(Body::empty())?)
        .await?;
    let status = response.status();
    let content_type = (response.headers().get(CONTENT_TYPE))
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let body = response.into_body().collect().await?.to_bytes();
    Ok((status, content_type, serde_json::from_slice(&body)?))
}

#[test]
fn names_turn_into_kebab_case() {
    assert_eq!(kebab("TodoNotFound"), "todo-not-found");
    assert_eq!(kebab("RequestRejected"), "request-rejected");
    assert_eq!(kebab("HTTPStatus"), "http-status");
    assert_eq!(kebab("H2Error"), "h2-error");
}

#[tokio::test]
async fn a_plain_text_rejection_becomes_a_problem_with_its_reason() -> TestResult {
    let (status, content_type, body) = get_json("/query?limit=ten").await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["type"], "about:blank");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.starts_with("Failed to deserialize query string"),
        "{detail}"
    );
    Ok(())
}

#[tokio::test]
async fn a_route_that_does_not_fit_api_path_is_this_services_mistake() -> TestResult {
    let (status, content_type, body) = get_json("/x/y").await?;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    assert_eq!(body["type"], "about:blank");
    assert!(body.get("detail").is_none(), "{body}");
    Ok(())
}

#[tokio::test]
async fn a_json_failure_answer_is_left_as_it_is() -> TestResult {
    let (status, content_type, body) = get_json("/json").await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    assert_eq!(body, json!({ "ready": false }));
    Ok(())
}
