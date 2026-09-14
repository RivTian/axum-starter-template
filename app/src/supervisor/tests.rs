use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[tokio::test]
async fn duplicate_registration_and_closed_registration_never_spawn_the_future() {
    let mut supervisor = TaskSupervisor::new();
    let runtime = Handle::current();
    supervisor
        .spawn_on("one", &runtime, "main", std::future::pending())
        .unwrap();
    let polled = Arc::new(AtomicBool::new(false));
    let marker = polled.clone();
    assert!(matches!(
        supervisor.spawn_on("one", &runtime, "main", async move {
            marker.store(true, Ordering::SeqCst);
            Ok(())
        }),
        Err(RegisterError::Duplicate("one"))
    ));
    supervisor.close_registration();
    assert!(matches!(
        supervisor.spawn_on("two", &runtime, "main", async { Ok(()) }),
        Err(RegisterError::Closed)
    ));
    assert!(!polled.load(Ordering::SeqCst));
    supervisor.abort_remaining();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), supervisor.next_exit())
            .await
            .unwrap()
            .unwrap()
            .kind,
        ExitKind::Cancelled
    ));
    assert!(supervisor.pending_names().is_empty());
}
#[tokio::test]
async fn actual_results_keep_names_for_return_error_and_panic() {
    let runtime = Handle::current();
    let mut supervisor = TaskSupervisor::new();
    supervisor
        .spawn_on("returned", &runtime, "main", async { Ok(()) })
        .unwrap();
    supervisor
        .spawn_on("failed", &runtime, "main", async {
            Err(std::io::Error::other("test failure").into())
        })
        .unwrap();
    supervisor
        .spawn_on("panicked", &runtime, "main", async {
            panic!("test task panic")
        })
        .unwrap();
    assert_eq!(supervisor.len(), 3);
    for _ in 0..3 {
        let exit = tokio::time::timeout(Duration::from_secs(1), supervisor.next_exit())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            exit.name,
            match exit.kind {
                ExitKind::Returned => "returned",
                ExitKind::Failed(_) => "failed",
                ExitKind::Panicked => "panicked",
                _ => panic!("wrong kind"),
            }
        );
    }
    assert!(supervisor.next_exit().await.is_none());
    assert!(supervisor.try_next_exit().is_none());
}
#[tokio::test]
async fn dropping_the_owner_requests_abort_of_the_real_task() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel::<()>();
    let mut supervisor = TaskSupervisor::new();
    supervisor
        .spawn_on("held", &Handle::current(), "main", async move {
            let _drop_sender = dropped_tx;
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
            Ok(())
        })
        .unwrap();
    entered_rx.await.unwrap();
    drop(supervisor);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .unwrap()
            .is_err()
    );
}
