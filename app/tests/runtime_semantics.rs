//! M1 proofs of Tokio primitives, not the completed M2 TaskSupervisor.

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const BUDGET: Duration = Duration::from_secs(5);

#[test]
fn a_joinset_on_main_owns_actual_tasks_spawned_on_another_runtime() {
    let main = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let extra = Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("m1-compute")
        .enable_all()
        .build()
        .unwrap();
    main.block_on(async {
        let mut tasks = JoinSet::new();
        let abort = tasks.spawn_on(
            async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                std::thread::current().name().unwrap().to_owned()
            },
            extra.handle(),
        );
        let (id, thread) = timeout(BUDGET, tasks.join_next_with_id())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(id, abort.id());
        assert_eq!(thread, "m1-compute");
        assert!(tasks.is_empty());
    });
    extra.shutdown_timeout(BUDGET);
    main.shutdown_timeout(BUDGET);
}

#[tokio::test]
async fn task_errors_and_panics_are_distinct_and_keep_the_original_id() {
    let mut tasks: JoinSet<io::Result<()>> = JoinSet::new();
    let failed = tasks.spawn(async { Err(io::Error::other("task error")) });
    let panicked = tasks.spawn(async { panic!("M1 controlled unit-test panic") });
    let mut errors = 0;
    let mut panics = 0;
    for _ in 0..2 {
        match timeout(BUDGET, tasks.join_next_with_id())
            .await
            .unwrap()
            .unwrap()
        {
            Ok((id, Err(error))) => {
                assert_eq!(id, failed.id());
                assert_eq!(error.to_string(), "task error");
                errors += 1;
            }
            Err(error) => {
                assert_eq!(error.id(), panicked.id());
                assert!(error.is_panic());
                panics += 1;
            }
            other => panic!("unexpected exit: {other:?}"),
        }
    }
    assert_eq!((errors, panics), (1, 1));
}

#[tokio::test]
async fn cooperative_async_abort_can_be_reaped_under_a_second_deadline() {
    let mut tasks = JoinSet::new();
    tasks.spawn(std::future::pending::<()>());
    assert!(
        timeout(Duration::from_millis(10), tasks.join_next())
            .await
            .is_err()
    );
    tasks.abort_all();
    assert!(
        timeout(BUDGET, tasks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(tasks.is_empty());
}

type Gate = Arc<(Mutex<bool>, Condvar)>;
struct ReleaseOnDrop(Gate);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_cannot_stop_already_started_blocking_work() {
    let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
    // Even a failed assertion releases the test's blocking worker before the
    // test runtime is dropped. The test must not itself create an orphan.
    let release = ReleaseOnDrop(gate.clone());
    let (started_tx, started_rx) = oneshot::channel();
    let mut tasks = JoinSet::new();
    tasks.spawn_blocking(move || {
        started_tx.send(()).unwrap();
        let (lock, changed) = &*gate;
        let mut allowed = lock.lock().unwrap();
        while !*allowed {
            allowed = changed.wait(allowed).unwrap();
        }
    });
    timeout(BUDGET, started_rx).await.unwrap().unwrap();
    tasks.abort_all();
    assert!(
        timeout(Duration::from_millis(20), tasks.join_next())
            .await
            .is_err()
    );
    drop(release);
    // It completed normally, not as a cancelled async task.
    timeout(BUDGET, tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[test]
fn children_cannot_cancel_their_parent_or_siblings() {
    let root = CancellationToken::new();
    let first = root.child_token();
    let second = root.child_token();
    first.cancel();
    assert!(!root.is_cancelled());
    assert!(!second.is_cancelled());
    root.cancel();
    assert!(second.is_cancelled());
}

#[tokio::test]
async fn an_empty_joinset_returns_none_without_waiting() {
    let mut tasks: JoinSet<()> = JoinSet::new();
    assert!(timeout(BUDGET, tasks.join_next()).await.unwrap().is_none());
}
