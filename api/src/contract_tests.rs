use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request as HttpRequest};
use axum::routing::{get, post};
use serde::Deserialize;
use service_core::{
    BuildInfo,
    lifecycle::{self, Phase},
};
use service_storage::{Storage, StorageError, StorageFuture};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;
use tower::ServiceExt;

#[derive(Clone)]
enum Mode {
    Healthy,
    Failed,
    Pending,
    Gated(Arc<Notify>, Arc<Notify>),
}
struct Fake {
    mode: Mode,
    calls: AtomicUsize,
}
impl Storage for Fake {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            match &self.mode {
                Mode::Healthy => Ok(()),
                Mode::Failed => Err(StorageError::unavailable()),
                Mode::Pending => std::future::pending().await,
                Mode::Gated(entered, release) => {
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                }
            }
        })
    }
}
fn setup(
    mode: Mode,
) -> (
    lifecycle::LifecyclePublisher,
    AppState,
    Arc<Fake>,
    HttpSettings,
) {
    let (writer, reader) = lifecycle::channel();
    let storage = Arc::new(Fake {
        mode,
        calls: AtomicUsize::new(0),
    });
    let state = AppState {
        storage: storage.clone(),
        lifecycle: reader,
        build: BuildInfo {
            service: "test-service",
            version: "1.0",
        },
    };
    let settings = HttpSettings::new(
        "127.0.0.1:0".parse().unwrap(),
        Duration::from_millis(100),
        Duration::from_millis(20),
    )
    .unwrap();
    (writer, state, storage, settings)
}
async fn body(response: Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
fn request(path: &str) -> HttpRequest<Body> {
    HttpRequest::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn health_and_info_do_not_query_storage_and_ready_requires_a_live_running_writer() {
    let (mut writer, state, storage, settings) = setup(Mode::Healthy);
    let router = build_router(state, &settings);
    for path in ["/v1/service/health", "/v1/service/info"] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(path))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(storage.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        router
            .clone()
            .oneshot(request("/v1/service/ready"))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    writer.publish(Phase::Running);
    let info = body(
        router
            .clone()
            .oneshot(request("/v1/service/info"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        info,
        serde_json::json!({"service":"test-service", "version":"1.0"})
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("/v1/service/ready"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    drop(writer);
    assert_eq!(
        router
            .oneshot(request("/v1/service/ready"))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(storage.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn storage_failure_and_probe_timeout_are_json_503() {
    for mode in [Mode::Failed, Mode::Pending] {
        let (mut writer, state, _, settings) = setup(mode);
        writer.publish(Phase::Running);
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            build_router(state, &settings).oneshot(request("/v1/service/ready")),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body(response).await["code"], 503);
    }
}

#[tokio::test]
async fn a_probe_that_crosses_draining_does_not_return_ready() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (mut writer, state, _, settings) = setup(Mode::Gated(entered.clone(), release.clone()));
    writer.publish(Phase::Running);
    let response = build_router(state, &settings).oneshot(request("/v1/service/ready"));
    let transition = async {
        entered.notified().await;
        writer.publish(Phase::Draining);
        release.notify_one();
    };
    let (response, ()) = tokio::join!(response, transition);
    assert_eq!(response.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn root_and_nested_misses_405_allow_and_head_preserve_the_contract() {
    let (_writer, state, _, settings) = setup(Mode::Healthy);
    let router = build_router(state, &settings);
    for path in ["/", "/v1", "/v1/", "/v1/unknown", "/elsewhere"] {
        let response = router.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body(response).await["code"], 404);
    }
    let response = router
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method(Method::POST)
                .uri("/v1/service/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let allow = response.headers().get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("GET") && allow.contains("HEAD"));
    assert_eq!(body(response).await["code"], 405);
    let response = router
        .oneshot(
            HttpRequest::builder()
                .method(Method::HEAD)
                .uri("/v1/service/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
}

#[derive(Deserialize)]
struct Input {
    id: u64,
}
fn extract_router() -> Router {
    Router::new()
        .route(
            "/json",
            post(|extract::Json(input): extract::Json<Input>| async move { input.id.to_string() }),
        )
        .route(
            "/path/{id}",
            get(|extract::Path(id): extract::Path<u64>| async move { id.to_string() }),
        )
        .route(
            "/query",
            get(|extract::Query(input): extract::Query<Input>| async move { input.id.to_string() }),
        )
        .route(
            "/missing-path",
            get(|extract::Path(id): extract::Path<u64>| async move { id.to_string() }),
        )
}

#[tokio::test]
async fn extractor_errors_keep_framework_statuses_without_leaking_values() {
    let router = extract_router();
    let cases = [
        ("/json", Method::POST, None, "{}".to_owned(), 415),
        (
            "/json",
            Method::POST,
            Some("application/json"),
            "{".to_owned(),
            400,
        ),
        (
            "/json",
            Method::POST,
            Some("application/json"),
            r#"{"id":"PRIVATE_TOKEN"}"#.to_owned(),
            422,
        ),
        (
            "/json",
            Method::POST,
            Some("application/json"),
            "x".repeat(2 * 1024 * 1024 + 1),
            413,
        ),
        ("/path/PRIVATE_TOKEN", Method::GET, None, String::new(), 400),
        (
            "/query?id=PRIVATE_TOKEN",
            Method::GET,
            None,
            String::new(),
            400,
        ),
        ("/missing-path", Method::GET, None, String::new(), 500),
    ];
    for (path, method, content_type, payload, status) in cases {
        let mut req = HttpRequest::builder().method(method).uri(path);
        if let Some(kind) = content_type {
            req = req.header("content-type", kind);
        }
        let response = router
            .clone()
            .oneshot(req.body(Body::from(payload)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{path}");
        let value = body(response).await;
        assert_eq!(value["code"], status);
        assert!(!value.to_string().contains("PRIVATE_TOKEN"));
        if status == 500 {
            assert_eq!(value["description"], "internal server error");
        }
    }
}

#[tokio::test]
async fn the_outer_request_deadline_uses_the_same_json_error_envelope() {
    let (_writer, state, _, settings) = setup(Mode::Healthy);
    let router = finish(
        Router::new().route("/pending", get(std::future::pending::<&'static str>)),
        state,
        &settings,
    );
    let response =
        tokio::time::timeout(Duration::from_secs(1), router.oneshot(request("/pending")))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["code"], 503);
}

#[tokio::test]
async fn listener_binding_does_not_open_the_startup_commit_gate() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut publisher, state, _, settings) = setup(Mode::Healthy);
    let cancel = CancellationToken::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(run(state, settings, cancel.child_token(), tx));
    let address = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .unwrap()
        .unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            b"GET /v1/service/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut first = [0_u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(20), stream.read(&mut first))
            .await
            .is_err()
    );
    publisher.publish(Phase::Running);
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
}
