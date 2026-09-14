use super::*;
use axum::extract::State;
use axum::routing::get;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Builder;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinSet;
use tokio::time::timeout;

const BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct HeldRequest {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn held_handler(State(state): State<HeldRequest>) -> &'static str {
    let _guard = DropFlag(state.dropped.clone());
    state.entered.notify_one();
    state.release.notified().await;
    "completed"
}

fn router(state: HeldRequest) -> Router {
    // This route exists only in tests, never in a generated service's HTTP API.
    Router::new()
        .route("/held", get(held_handler))
        .with_state(state)
}

async fn start_request(address: std::net::SocketAddr, state: &HeldRequest) -> TcpStream {
    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"GET /held HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    state.entered.notified().await;
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_completion_waits_for_the_request_to_finish() {
    timeout(BUDGET, async {
        let state = HeldRequest::default();
        let root = CancellationToken::new();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut tasks = JoinSet::new();
        tasks.spawn(serve(listener, router(state.clone()), root.child_token()));
        let mut client = start_request(address, &state).await;
        root.cancel();
        assert!(tasks.try_join_next().is_none());
        assert!(!state.dropped.load(Ordering::SeqCst));
        state.release.notify_one();
        tasks.join_next().await.unwrap().unwrap().unwrap();
        assert!(state.dropped.load(Ordering::SeqCst));
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with("completed"));
    })
    .await
    .expect("HTTP graceful probe exceeded its deadline");
}

#[test]
fn aborting_the_outer_serve_task_does_not_reap_the_connection_task() {
    // Both runtime owners stay on a synchronous stack, never inside async Drop.
    let main = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let extra = Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("test-http-extra")
        .enable_all()
        .build()
        .unwrap();
    let state = HeldRequest::default();
    let client = main.block_on(async {
        timeout(BUDGET, async {
            let root = CancellationToken::new();
            let (bound_tx, bound_rx) = oneshot::channel();
            let mut tasks = JoinSet::new();
            let http_state = state.clone();
            let child = root.child_token();
            let abort = tasks.spawn_on(
                async move {
                    // bind/registration happen in the target runtime, not the parent.
                    let listener = TcpListener::bind("127.0.0.1:0").await?;
                    bound_tx.send(listener.local_addr()?).unwrap();
                    serve(listener, router(http_state), child).await
                },
                extra.handle(),
            );
            let address = bound_rx.await.unwrap();
            let client = start_request(address, &state).await;
            root.cancel();
            abort.abort();
            let error = tasks.join_next().await.unwrap().unwrap_err();
            assert!(error.is_cancelled());
            // This is the critical negative proof: the supervisor's join is not
            // evidence that the library-owned handler has been reaped.
            assert!(!state.dropped.load(Ordering::SeqCst));
            client
        })
        .await
        .expect("HTTP forced probe exceeded its deadline")
    });
    // The connection is still held; no release notification is sent. Shutting
    // down its actual runtime, rather than just serve, drops the pending handler.
    extra.shutdown_timeout(BUDGET);
    assert!(state.dropped.load(Ordering::SeqCst));
    drop(client);
    main.shutdown_timeout(BUDGET);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_panic_closes_its_connection_not_the_top_level_http_task() {
    timeout(BUDGET, async {
        let root = CancellationToken::new();
        let entered = Arc::new(Notify::new());
        let notify = entered.clone();
        let router = Router::new()
            .route(
                "/panic",
                get(move || {
                    let notify = notify.clone();
                    async move {
                        notify.notify_one();
                        panic!("test-only request panic");
                        #[allow(unreachable_code)]
                        "never returned"
                    }
                }),
            )
            .route("/alive", get(|| async { "still serving" }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut tasks = JoinSet::new();
        tasks.spawn(serve(listener, router, root.child_token()));
        let mut failed = TcpStream::connect(address).await.unwrap();
        failed
            .write_all(b"GET /panic HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        entered.notified().await;
        let mut bytes = Vec::new();
        // EOF or reset is allowed. A request panic is not promised a JSON 500.
        let _ = failed.read_to_end(&mut bytes).await;
        assert!(
            tasks.try_join_next().is_none(),
            "request panic escaped to the HTTP face"
        );
        let mut healthy = TcpStream::connect(address).await.unwrap();
        healthy
            .write_all(b"GET /alive HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        healthy.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("200 OK") && response.ends_with("still serving"));
        root.cancel();
        tasks.join_next().await.unwrap().unwrap().unwrap();
    })
    .await
    .expect("request panic isolation probe exceeded deadline");
}
