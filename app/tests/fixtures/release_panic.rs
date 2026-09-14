//! Standalone release regression fixture for the actual TaskSupervisor.
//! The checker expects panic observation, sibling cleanup and a failure exit;
//! this executable is not a service example or a production panic switch.

#[cfg(not(panic = "unwind"))]
compile_error!("the release panic probe requires panic=unwind");

#[path = "../../src/supervisor.rs"]
mod supervisor;

use service_core::TickerInterval;
use std::process::ExitCode;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use supervisor::{ExitKind, TaskError, TaskSupervisor};
use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const BUDGET: Duration = Duration::from_secs(5);
struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn probe(extra: &Handle) {
    let (mut lifecycle, reader) = service_core::lifecycle::channel();
    let configuration =
        service_core::config::ConfigPublisher::new(service_core::config::HotConfig {
            ticker: TickerInterval::try_from(Duration::from_secs(1)).unwrap(),
        });
    let config_reader = configuration.subscribe();
    let root = CancellationToken::new();
    let child = root.child_token();
    let dropped = Arc::new(AtomicBool::new(false));
    let worker_dropped = dropped.clone();
    let (started_tx, started_rx) = oneshot::channel();
    let mut tasks = TaskSupervisor::new();
    tasks
        .spawn_on("ticker", extra, "compute", async move {
            let _guard = Dropped(worker_dropped);
            service_worker::run(config_reader, reader, child, started_tx)
                .await
                .map_err(|error| Box::new(error) as TaskError)
        })
        .unwrap();
    timeout(BUDGET, started_rx).await.unwrap().unwrap();
    lifecycle.publish(service_core::lifecycle::Phase::Running);
    assert_eq!(tasks.len(), 1);
    assert!(tasks.try_next_exit().is_none());
    tasks
        .spawn_on("panic", extra, "compute", async {
            panic!("controlled release task panic")
        })
        .unwrap();
    let exited = timeout(BUDGET, tasks.next_exit()).await.unwrap().unwrap();
    assert_eq!(exited.name, "panic");
    assert_eq!(exited.runtime, "compute");
    assert_eq!(exited.kind.label(), "panicked");
    match exited.kind {
        ExitKind::Panicked => {}
        ExitKind::Failed(error) => panic!("unexpected task error: {error}"),
        other => panic!("unexpected exit: {other:?}"),
    }
    println!("release-probe:panic-observed");
    tasks.close_registration();
    root.cancel();
    let worker = match timeout(BUDGET, tasks.next_exit()).await {
        Ok(Some(worker)) => worker,
        _ => {
            tasks.abort_remaining();
            let _ = timeout(BUDGET, tasks.next_exit()).await;
            panic!("worker failed to drain");
        }
    };
    assert_eq!(worker.name, "ticker");
    assert_eq!(worker.runtime, "compute");
    assert!(worker.kind.returned());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(tasks.is_empty());
    assert!(tasks.pending_names().is_empty());
    println!("release-probe:worker-joined");
}

fn main() -> ExitCode {
    let main = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let extra = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    main.block_on(probe(extra.handle()));
    extra.shutdown_timeout(BUDGET);
    main.shutdown_timeout(BUDGET);
    println!("release-probe:runtime-teardown-returned");
    ExitCode::FAILURE
}
