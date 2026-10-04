//! The HTTP contract through the router: every success shape, every kind of failure as a
//! problem document, the request id, the probes, and how a failed request is logged.

use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use svc_api::router::router;
use svc_api::settings::ServerSettings;
use svc_api::state::AppState;
use svc_domain::todo::TodoUseCases;
use svc_runtime::prelude::*;
use svc_test_utils::clock::FakeClock;
use svc_test_utils::logs::CapturedLogs;
use svc_test_utils::publisher::RecordingPublisher;
use svc_test_utils::repo::FakeTodoRepository;
use svc_util::prelude::*;
use tower::ServiceExt;

type TestResult = std::result::Result<(), Box<dyn StdError>>;

struct Api {
    router: Router,
    repository: Arc<FakeTodoRepository>,
}

fn api() -> Api {
    let repository = Arc::new(FakeTodoRepository::default());
    let todos = TodoUseCases::new(
        repository.clone(),
        Arc::new(RecordingPublisher::default()),
        Arc::new(FakeClock::default()),
    );
    let readiness = Readiness::new(PhaseWatch::new(), HealthRegistry::default());
    let state = AppState::new(Arc::new(todos), readiness, "acme-svc");
    let settings = ServerSettings {
        request_timeout: Duration::from_secs(1),
        body_limit_bytes: 64,
        ..ServerSettings::default()
    };
    Api {
        router: router(state, &settings),
        repository,
    }
}

/// A response: status, headers and the body as JSON (`null` when empty).
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

impl Api {
    async fn send(&self, request: Request<Body>) -> std::result::Result<Answer, Box<dyn StdError>> {
        let response = self.router.clone().oneshot(request).await?;
        let (parts, body) = response.into_parts();
        let bytes = body.collect().await?.to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        };
        Ok(Answer {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> std::result::Result<Answer, Box<dyn StdError>> {
        let request = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?,
            None => request.body(Body::empty())?,
        };
        self.send(request).await
    }

    async fn create(&self, title: &str) -> std::result::Result<Answer, Box<dyn StdError>> {
        let body = json!({ "title": title });
        self.call(Method::POST, "/v1/todos", Some(body)).await
    }
}

/// The members of a problem document other than `request_id`, which the caller checks.
fn problem(kind: &str, title: &str, status: u16, detail: Option<&str>, instance: &str) -> Value {
    let mut expected =
        json!({ "type": kind, "title": title, "status": status, "instance": instance });
    if let (Some(detail), Some(map)) = (detail, expected.as_object_mut()) {
        map.insert("detail".into(), detail.into());
    }
    expected
}

/// The problem without its `request_id`, after checking it repeats the response header.
fn without_request_id(answer: &Answer) -> std::result::Result<Value, Box<dyn StdError>> {
    assert_eq!(
        answer.header("content-type"),
        Some("application/problem+json")
    );
    let mut body = answer.body.clone();
    let map = body.as_object_mut().ok_or("the problem is not an object")?;
    let id = map.remove("request_id").ok_or("no request_id")?;
    assert_eq!(id.as_str(), answer.header("x-request-id"));
    Ok(body)
}

#[tokio::test]
async fn the_success_shapes() -> TestResult {
    let api = api();
    let created = api.create("Buy milk").await?;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.body["id"].as_str().ok_or("no id")?.to_string();
    assert_eq!(
        created.header("location"),
        Some(format!("/v1/todos/{id}").as_str())
    );
    let todo = json!({ "id": id, "title": "Buy milk", "done": false, "version": 1 });
    assert_eq!(created.body, todo);

    let got = api
        .call(Method::GET, &format!("/v1/todos/{id}"), None)
        .await?;
    assert_eq!((got.status, got.body), (StatusCode::OK, todo.clone()));
    let listed = api.call(Method::GET, "/v1/todos?limit=10", None).await?;
    assert_eq!(
        (listed.status, listed.body),
        (StatusCode::OK, json!([todo]))
    );

    let uri = format!("/v1/todos/{id}/complete");
    let done = api
        .call(Method::POST, &uri, Some(json!({ "version": 1 })))
        .await?;
    assert_eq!(
        (done.status, &done.body["version"]),
        (StatusCode::OK, &json!(2))
    );

    let deleted = api
        .call(Method::DELETE, &format!("/v1/todos/{id}"), None)
        .await?;
    assert_eq!(
        (deleted.status, deleted.body),
        (StatusCode::NO_CONTENT, Value::Null)
    );
    Ok(())
}

#[tokio::test]
async fn client_errors_of_an_error_kind_name_it() -> TestResult {
    let api = api();
    let invalid = api.create("").await?;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        without_request_id(&invalid)?,
        problem(
            "urn:acme-svc:problem:invalid-todo-title",
            "Invalid todo title",
            422,
            Some("todo title must not be empty"),
            "/v1/todos"
        )
    );

    let id = "0199a8f0-0000-7000-8000-000000000000";
    let missing = api
        .call(Method::GET, &format!("/v1/todos/{id}"), None)
        .await?;
    assert_eq!(missing.body["type"], "urn:acme-svc:problem:todo-not-found");
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let created = api.create("Buy milk").await?;
    let uri = format!(
        "/v1/todos/{}/complete",
        created.body["id"].as_str().ok_or("no id")?
    );
    let stale = api
        .call(Method::POST, &uri, Some(json!({ "version": 7 })))
        .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.body["detail"], "expected version 7, found 1");
    Ok(())
}

#[tokio::test]
async fn malformed_requests_are_problems_too() -> TestResult {
    let api = api();
    let syntax = Request::post("/v1/todos")
        .header("content-type", "application/json")
        .body(Body::from("{"))?;
    let shape = api
        .call(Method::POST, "/v1/todos", Some(json!({ "name": "x" })))
        .await?;
    let no_type = Request::post("/v1/todos").body(Body::from("{}"))?;
    let too_large = api.create(&"x".repeat(100)).await?;
    let bad_id = api.call(Method::GET, "/v1/todos/not-a-uuid", None).await?;
    let bad_limit = api.call(Method::GET, "/v1/todos?limit=0", None).await?;
    let unknown = api.call(Method::GET, "/nowhere", None).await?;
    let method = api.call(Method::PUT, "/v1/todos", None).await?;
    let answers = [
        (api.send(syntax).await?, 400, "about:blank"),
        (shape, 422, "urn:acme-svc:problem:request-rejected"),
        (api.send(no_type).await?, 415, "about:blank"),
        (too_large, 413, "about:blank"),
        (bad_id, 400, "about:blank"),
        (bad_limit, 400, "about:blank"),
        (unknown, 404, "about:blank"),
        (method, 405, "about:blank"),
    ];
    for (answer, status, kind) in &answers {
        let body = without_request_id(answer)?;
        assert_eq!(
            (answer.status.as_u16(), &body["type"]),
            (*status, &json!(kind))
        );
    }
    assert_eq!(answers[7].0.header("allow"), Some("GET,HEAD,POST"));
    assert_eq!(
        answers[4].0.body["detail"]
            .as_str()
            .map(|d| d.starts_with("invalid path")),
        Some(true)
    );
    Ok(())
}

#[tokio::test]
async fn server_errors_show_no_detail() -> TestResult {
    let api = api();
    api.repository.fail_with(ErrorType::ConnectRefused);
    let unavailable = api.create("Buy milk").await?;
    assert_eq!(
        without_request_id(&unavailable)?,
        problem("about:blank", "Service Unavailable", 503, None, "/v1/todos")
    );
    assert_eq!(unavailable.header("retry-after"), None);
    api.repository.fail_with(ErrorType::InternalError);
    let internal = api.call(Method::GET, "/v1/todos", None).await?;
    assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(internal.body.get("detail"), None);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_slow_request_times_out_with_503_and_retry_after() -> TestResult {
    let api = api();
    api.repository.slow_down(Duration::from_secs(5));
    let answer = api.call(Method::GET, "/v1/todos", None).await?;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.header("retry-after"), Some("1"));
    Ok(())
}

#[tokio::test]
async fn a_panicking_handler_answers_500() -> TestResult {
    let api = api();
    api.repository.panic_on_call();
    let answer = api.call(Method::GET, "/v1/todos", None).await?;
    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(without_request_id(&answer)?["type"], "about:blank");
    Ok(())
}

#[tokio::test]
async fn a_safe_request_id_is_kept_and_any_other_is_replaced() -> TestResult {
    let api = api();
    let send = |id: &str| {
        Request::get("/livez")
            .header("x-request-id", id)
            .body(Body::empty())
    };
    let kept = api.send(send("abc-123_x.y")?).await?;
    assert_eq!(kept.header("x-request-id"), Some("abc-123_x.y"));
    let replaced = api.send(send("has space")?).await?;
    let id = replaced.header("x-request-id").ok_or("no request id")?;
    assert!(id != "has space" && id.len() == 36);
    Ok(())
}

#[tokio::test]
async fn the_probes() -> TestResult {
    let api = api();
    let live = api.call(Method::GET, "/livez", None).await?;
    assert_eq!(
        (live.status, live.body),
        (StatusCode::OK, json!({ "status": "live" }))
    );
    let ready = api.call(Method::GET, "/readyz", None).await?;
    let starting = json!({ "status": "not ready", "phase": "starting", "checks": {} });
    assert_eq!(
        (ready.status, ready.body),
        (StatusCode::SERVICE_UNAVAILABLE, starting)
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_request_is_logged_once_with_the_error_fields() -> TestResult {
    let (logs, _guard) = CapturedLogs::start();
    let api = api();
    api.repository.fail_with(ErrorType::ConnectRefused);
    api.create("Buy milk").await?;
    let failed = logs.with_message("request failed");
    assert_eq!(failed.len(), 1);
    let event = &failed[0];
    let fields = [
        "level",
        "error.type",
        "error.class",
        "error.source",
        "http.response.status_code",
    ];
    let values: Vec<&Value> = fields.iter().filter_map(|key| event.get(*key)).collect();
    assert_eq!(
        values,
        [
            &json!("WARN"),
            &json!("ConnectRefused"),
            &json!("Unavailable"),
            &json!("upstream"),
            &json!(503)
        ]
    );
    let finished = logs.with_message("request finished");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].get("target"), Some(&json!("svc_api::access")));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_request_the_client_gives_up_on_is_still_logged() -> TestResult {
    let (logs, _guard) = CapturedLogs::start();
    let api = api();
    api.repository.slow_down(Duration::from_millis(500));
    let request = Request::get("/v1/todos").body(Body::empty())?;
    let gave_up = tokio::time::timeout(Duration::from_millis(100), api.send(request)).await;
    assert!(gave_up.is_err());
    let finished = logs.with_message("request finished");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].get("cancelled"), Some(&json!(true)));
    Ok(())
}

#[tokio::test]
async fn a_path_that_is_not_utf8_gets_a_fixed_reason() -> TestResult {
    let answer = api().call(Method::GET, "/v1/todos/%FF", None).await?;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body["detail"],
        "invalid path parameter: not valid UTF-8 text"
    );
    Ok(())
}
