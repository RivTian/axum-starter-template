use super::contract_tests::{Mode, setup_with};
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

/// Read until the response text a case is waiting for has arrived, rather than
/// to EOF: these requests are keep-alive, so there is no EOF to wait for.
async fn read_until(stream: &mut TcpStream, sentinel: &str) -> String {
    let mut seen = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(
            read > 0,
            "the peer closed before sending {sentinel:?}; got {:?}",
            String::from_utf8_lossy(&seen)
        );
        seen.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&seen);
        if text.contains(sentinel) {
            return text.into_owned();
        }
    }
}

/// A panicking handler is answered, not hung up on, and it costs the caller
/// neither its connection nor the HTTP face.
///
/// The envelope itself is pinned in `contract_tests`; what only a real socket
/// can show is the other half -- that the 500 reaches the wire ahead of any
/// reset, that the same connection then serves a second request, and that the
/// top-level task the supervisor joins never sees the unwind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_panic_is_answered_in_the_envelope_and_keeps_its_connection() {
    timeout(BUDGET, async {
        let root = CancellationToken::new();
        // Assembled through `finish`, so what this observes on the wire is what
        // a generated service actually sends.
        let (_publisher, state, _, settings) =
            setup_with(Mode::Healthy, BUDGET, Duration::from_secs(1));
        let router = finish(
            Router::new()
                .route(
                    "/panic",
                    get(|| async {
                        panic!("test-only request panic");
                        #[allow(unreachable_code)]
                        "never returned"
                    }),
                )
                .route("/alive", get(|| async { "still serving" })),
            state,
            &settings,
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut tasks = JoinSet::new();
        tasks.spawn(serve(listener, router, root.child_token()));
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"GET /panic HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let failure = read_until(&mut client, "internal server error").await;
        assert!(failure.starts_with("HTTP/1.1 500"), "{failure:?}");
        assert!(
            tasks.try_join_next().is_none(),
            "request panic escaped to the HTTP face"
        );
        // Same socket, no reconnect: the unwind did not take the connection
        // with it, so a client is not forced to redial after one bad route.
        client
            .write_all(b"GET /alive HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let alive = read_until(&mut client, "still serving").await;
        assert!(alive.contains("200 OK"), "{alive:?}");
        root.cancel();
        drop(client);
        tasks.join_next().await.unwrap().unwrap().unwrap();
    })
    .await
    .expect("request panic isolation probe exceeded deadline");
}
