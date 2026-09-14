use super::*;
use crate::shutdown::StorageClose;
use crate::signals::ControlClosed;
use crate::supervisor::ExitKind;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

fn config(dir: &std::path::Path) -> Config {
    let path = dir.join("service.toml");
    std::fs::write(
        &path,
        "[storage]\npath='data/service.sqlite3'\n[runtime]\nworker_threads=2",
    )
    .unwrap();
    let context = crate::config::LoadContext::new(path, dir, BTreeMap::new(), 2).unwrap();
    crate::config::load(&context).unwrap()
}
async fn run<C, S>(
    config: &Config,
    build: BuildInfo,
    context: &mut ShutdownContext,
    control: C,
    on_started: S,
) -> RunReport
where
    C: AsyncFnMut() -> ControlResult,
    S: FnOnce(SocketAddr),
{
    let candidate = config.clone();
    run_with_loader(config, build, context, control, on_started, move || {
        Ok(candidate.clone())
    })
    .await
}

async fn run_with_loader<C, S, L>(
    config: &Config,
    build: BuildInfo,
    context: &mut ShutdownContext,
    control: C,
    on_started: S,
    loader: L,
) -> RunReport
where
    C: AsyncFnMut() -> ControlResult,
    S: FnOnce(SocketAddr),
    L: Fn() -> Result<Config, ConfigError> + Send + Sync + 'static,
{
    let executors = Executors::current();
    super::run(
        config, build, context, control, on_started, loader, &executors,
    )
    .await
}

fn build() -> BuildInfo {
    BuildInfo {
        service: "test-service",
        version: "1",
    }
}
async fn get(address: SocketAddr, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut result = String::new();
    stream.read_to_string(&mut result).await.unwrap();
    result
}

#[tokio::test]
async fn two_assemblies_in_one_process_have_independent_state_and_shutdown() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let left_dir = tempfile::tempdir().unwrap();
        let right_dir = tempfile::tempdir().unwrap();
        let left_cfg = config(left_dir.path());
        let right_cfg = config(right_dir.path());
        let mut left_context = ShutdownContext::default();
        let mut right_context = ShutdownContext::default();
        let (left_tx, mut left_rx) = mpsc::channel(4);
        let (right_tx, mut right_rx) = mpsc::channel(4);
        let (left_ready_tx, left_ready_rx) = oneshot::channel();
        let (right_ready_tx, right_ready_rx) = oneshot::channel();
        let left = run(
            &left_cfg,
            build(),
            &mut left_context,
            async || left_rx.recv().await.ok_or(ControlClosed),
            |address| left_ready_tx.send(address).unwrap(),
        );
        let right = run(
            &right_cfg,
            build(),
            &mut right_context,
            async || right_rx.recv().await.ok_or(ControlClosed),
            |address| right_ready_tx.send(address).unwrap(),
        );
        let requests = async {
            let a = left_ready_rx.await.unwrap();
            let b = right_ready_rx.await.unwrap();
            assert_ne!(a, b);
            assert!(
                get(a, "/v1/service/ready")
                    .await
                    .starts_with("HTTP/1.1 200")
            );
            left_tx.send(Event::Stop).await.unwrap();
            assert!(
                get(b, "/v1/service/ready")
                    .await
                    .starts_with("HTTP/1.1 200")
            );
            right_tx.send(Event::Stop).await.unwrap();
        };
        let (left, right, ()) = tokio::join!(left, right, requests);
        assert!(left.succeeded() && right.succeeded());
        assert_eq!(left.storage, StorageClose::Closed);
        assert_eq!(right.storage, StorageClose::Closed);
        assert_eq!(left.tasks.len(), 2);
        assert_eq!(right.tasks.len(), 2);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_stop_ready_before_boot_cancels_before_creating_data_resources() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let mut context = ShutdownContext::default();
    let (tx, mut rx) = mpsc::channel(2);
    tx.send(Event::Stop).await.unwrap();
    let report = run(
        &cfg,
        build(),
        &mut context,
        async || rx.recv().await.ok_or(ControlClosed),
        |_| panic!("startup was committed"),
    )
    .await;
    assert!(report.succeeded());
    assert_eq!(report.storage, StorageClose::NotCreated);
    assert!(!dir.path().join("data").exists());
}

#[tokio::test]
async fn bind_failure_retains_the_task_error_instead_of_the_ack_channel_error() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path());
    cfg.boot.http = service_api::HttpSettings::new(
        occupied.local_addr().unwrap(),
        Duration::from_secs(2),
        Duration::from_millis(500),
    )
    .unwrap();
    let mut context = ShutdownContext::default();
    let (_tx, mut rx) = mpsc::channel(2);
    let report = run(
        &cfg,
        build(),
        &mut context,
        async || rx.recv().await.ok_or(ControlClosed),
        |_| panic!("bind failure committed startup"),
    )
    .await;
    match &report.cause {
        StopCause::Task(TaskExit {
            name: "http",
            runtime: _,
            kind: ExitKind::Failed(error),
        }) => assert!(matches!(
            error.downcast_ref::<service_api::ServeError>(),
            Some(service_api::ServeError::Bind(_))
        )),
        other => panic!("lost bind error: {other:?}"),
    }
    assert_eq!(report.storage, StorageClose::Closed);
    assert!(
        report
            .tasks
            .iter()
            .any(|task| task.name == "ticker" && task.kind.returned())
    );
    assert!(!report.succeeded());
}

fn fake_resources(context: &mut ShutdownContext) -> Resources<'_> {
    let dir = tempfile::tempdir().unwrap();
    let candidate = config(dir.path());
    let initial = candidate.clone();
    let reload = ReloadController::new(&initial, move || Ok(candidate.clone()));
    let (mut writer, _) = lifecycle::channel();
    writer.publish(Phase::Running);
    Resources {
        gate: Gate {
            writer,
            root: CancellationToken::new(),
            context,
            budget: Budgets {
                grace: Duration::from_millis(20),
                abort_reap: Duration::from_millis(200),
                storage_close: Duration::from_secs(1),
                runtime: Duration::from_secs(1),
            },
            events_open: true,
        },
        supervisor: TaskSupervisor::new(),
        reload,
        storage: None,
        committed: true,
        finished: false,
    }
}

#[tokio::test]
async fn forcing_http_skips_normal_pool_close_even_after_the_outer_join() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    std::fs::create_dir_all(cfg.boot.storage.path().parent().unwrap()).unwrap();
    let owner = StorageOwner::prepare(cfg.boot.storage.clone());
    let facade = owner.initialize().await.unwrap();
    let mut context = ShutdownContext::default();
    let mut resources = fake_resources(&mut context);
    resources.storage = Some(owner);
    resources
        .supervisor
        .spawn_on("http", &Handle::current(), "main", std::future::pending())
        .unwrap();
    resources.gate.stop(StopCause::Signal);
    let (_tx, mut rx) = mpsc::channel(1);
    let report = cleanup(&mut resources, &mut async || {
        rx.recv().await.ok_or(ControlClosed)
    })
    .await;
    resources.finished = true;
    assert!(report.forced);
    assert_eq!(report.storage, StorageClose::SkippedUnproven);
    assert!(matches!(report.tasks[0].kind, ExitKind::Cancelled));
    facade.health().await.unwrap(); // the normal close branch really was not executed
    resources.storage.as_ref().unwrap().close().await;
    assert!(!report.succeeded());
}

#[tokio::test]
async fn second_stop_escalates_without_refreshing_the_timeline() {
    let mut context = ShutdownContext::default();
    let mut resources = fake_resources(&mut context);
    resources.gate.budget.grace = Duration::from_secs(60);
    resources
        .supervisor
        .spawn_on("http", &Handle::current(), "main", std::future::pending())
        .unwrap();
    resources.gate.stop(StopCause::Signal);
    let first = resources.gate.context.plan.unwrap();
    let (tx, mut rx) = mpsc::channel(2);
    tx.send(Event::Stop).await.unwrap();
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        cleanup(&mut resources, &mut async || {
            rx.recv().await.ok_or(ControlClosed)
        }),
    )
    .await
    .unwrap();
    resources.finished = true;
    assert!(report.forced);
    assert!(!report.succeeded());
    assert_eq!(
        resources.gate.context.plan.unwrap().final_deadline,
        first.final_deadline
    );
}

#[tokio::test]
async fn task_failure_wins_over_an_already_queued_stop() {
    let mut context = ShutdownContext::default();
    let mut resources = fake_resources(&mut context);
    // On this current-thread test runtime, the task has no await after send;
    // Tokio publishes completion before the receiver can resume.
    let (done_tx, done_rx) = oneshot::channel();
    resources
        .supervisor
        .spawn_on("http", &Handle::current(), "main", async move {
            done_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
    done_rx.await.unwrap();
    let (tx, mut rx) = mpsc::channel(2);
    tx.send(Event::Stop).await.unwrap();
    monitor(&mut resources, &mut async || {
        rx.recv().await.ok_or(ControlClosed)
    })
    .await;
    assert!(matches!(
        resources.gate.context.cause,
        Some(StopCause::Task(_))
    ));
    let report = cleanup(&mut resources, &mut async || {
        rx.recv().await.ok_or(ControlClosed)
    })
    .await;
    resources.finished = true;
    assert!(!report.succeeded());
}

#[tokio::test]
async fn closed_control_is_a_failure_not_a_busy_loop() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let mut context = ShutdownContext::default();
    let report = run(
        &cfg,
        build(),
        &mut context,
        async || Err(ControlClosed),
        |_| panic!("unexpected commit"),
    )
    .await;
    assert!(matches!(report.cause, StopCause::ControlClosed));
    assert!(!report.succeeded());
}

#[test]
fn coordinator_unwind_cancels_real_tasks_and_preserves_the_external_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let mut runtime = crate::rt::RuntimeSet::build(&cfg.boot).unwrap();
    let mut context = ShutdownContext::default();
    let (_tx, mut rx) = mpsc::channel(2);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(run(
            &cfg,
            build(),
            &mut context,
            async || rx.recv().await.ok_or(ControlClosed),
            |_| panic!("controlled coordinator panic"),
        ))
    }));
    assert!(result.is_err());
    assert!(matches!(context.cause, Some(StopCause::CoordinatorPanic)));
    assert_eq!(context.emergency_unreaped, ["http", "ticker"]);
    let deadline = context.plan.unwrap().final_deadline;
    assert!(context.runtime_deadline(cfg.boot.shutdown.runtime) <= deadline);
    assert!(!runtime.shutdown(context.runtime_deadline(cfg.boot.shutdown.runtime)));
}

#[tokio::test]
async fn close_timeout_does_not_replace_the_original_failure() {
    let mut context = ShutdownContext::default();
    let mut resources = fake_resources(&mut context);
    resources.gate.stop(StopCause::Startup {
        phase: "original",
        source: std::io::Error::other("original failure").into(),
    });
    let initial = resources.gate.context.plan.unwrap();
    let (_tx, mut rx) = mpsc::channel(1);
    let (outcome, interrupted, _) = close_storage(
        std::future::pending(),
        tokio::time::Instant::now() + Duration::from_millis(20),
        &mut resources.gate,
        &mut async || rx.recv().await.ok_or(ControlClosed),
    )
    .await;
    assert_eq!(outcome, StorageClose::TimedOut);
    assert!(!interrupted);
    assert!(matches!(
        &resources.gate.context.cause,
        Some(StopCause::Startup {
            phase: "original",
            ..
        })
    ));
    assert_eq!(
        resources.gate.context.plan.unwrap().final_deadline,
        initial.final_deadline
    );
    resources.finished = true;
}

#[test]
fn noncooperative_task_is_reported_unreaped_after_both_bounded_waits() {
    use std::sync::{Arc, Condvar, Mutex};
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let block = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(block.clone());
    let mut context = ShutdownContext::default();
    let report = runtime.block_on(async {
        let mut resources = fake_resources(&mut context);
        resources.gate.budget.abort_reap = Duration::from_millis(20);
        let (tx, rx) = oneshot::channel();
        resources
            .supervisor
            .spawn_on("noncooperative", &Handle::current(), "main", async move {
                tx.send(()).unwrap();
                // Deliberate fault injection: this task does not yield to abort.
                let (lock, wake) = &*block;
                let mut allowed = lock.lock().unwrap();
                while !*allowed {
                    allowed = wake.wait(allowed).unwrap();
                }
                Ok(())
            })
            .unwrap();
        rx.await.unwrap();
        resources.gate.stop(StopCause::Signal);
        let (_events_tx, mut events_rx) = mpsc::channel(1);
        let report = tokio::time::timeout(
            Duration::from_secs(2),
            cleanup(&mut resources, &mut async || {
                events_rx.recv().await.ok_or(ControlClosed)
            }),
        )
        .await
        .unwrap();
        resources.finished = true;
        report
    });
    // Release even on a failed assertion, before the test runtime is destroyed.
    drop(release);
    runtime.shutdown_timeout(Duration::from_secs(2));
    assert_eq!(report.unreaped, ["noncooperative"]);
    assert!(report.forced && report.cleanup_failed);
    assert!(!report.succeeded());
}

#[tokio::test]
async fn missing_confirmation_without_task_exit_remains_bounded_by_startup_deadline() {
    let mut context = ShutdownContext::default();
    let mut resources = fake_resources(&mut context);
    resources.committed = false;
    resources
        .supervisor
        .spawn_on("http", &Handle::current(), "main", std::future::pending())
        .unwrap();
    let (ack, mut receiver) = oneshot::channel::<SocketAddr>();
    drop(ack);
    let (_tx, mut rx) = mpsc::channel(1);
    let confirmed = confirmed(
        &mut receiver,
        "http_confirmation",
        tokio::time::Instant::now() + Duration::from_millis(20),
        &mut resources.supervisor,
        &mut resources.gate,
        &mut async || rx.recv().await.ok_or(ControlClosed),
    )
    .await;
    assert!(confirmed.is_none());
    assert!(resources.gate.root.is_cancelled());
    assert!(matches!(
        resources.gate.context.cause,
        Some(StopCause::StartupTimeout("http_confirmation"))
    ));
    let report = cleanup(&mut resources, &mut async || {
        rx.recv().await.ok_or(ControlClosed)
    })
    .await;
    resources.finished = true;
    assert!(!report.succeeded());
}

#[test]
fn a_slow_reload_keeps_http_and_stop_responsive_and_is_reported_unreaped() {
    use std::sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path());
    cfg.boot.http = service_api::HttpSettings::new(
        "127.0.0.1:0".parse().unwrap(),
        Duration::from_millis(20),
        Duration::from_millis(10),
    )
    .unwrap();
    cfg.boot.shutdown.grace = Duration::from_millis(50);
    cfg.boot.shutdown.abort_reap = Duration::from_millis(50);
    cfg.boot.reload_timeout = Duration::from_secs(1);
    let mut runtime = crate::rt::RuntimeSet::build(&cfg.boot).unwrap();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(gate.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (load_started_tx, load_started_rx) = oneshot::channel();
    let started = Mutex::new(Some(load_started_tx));
    let loaded = cfg.clone();
    let loader = move || {
        count.fetch_add(1, Ordering::SeqCst);
        started.lock().unwrap().take().unwrap().send(()).unwrap();
        let (lock, wake) = &*gate;
        let mut allowed = lock.lock().unwrap();
        while !*allowed {
            allowed = wake.wait(allowed).unwrap();
        }
        Ok(loaded.clone())
    };
    let mut context = ShutdownContext::default();
    let report = runtime.block_on(async {
        let (events_tx, mut events_rx) = mpsc::channel(16);
        let (ready_tx, ready_rx) = oneshot::channel();
        let running = run_with_loader(
            &cfg,
            build(),
            &mut context,
            async || events_rx.recv().await.ok_or(ControlClosed),
            |address| ready_tx.send(address).unwrap(),
            loader,
        );
        let exercise = async {
            let address = ready_rx.await.unwrap();
            events_tx.send(Event::Reload).await.unwrap();
            load_started_rx.await.unwrap();
            for _ in 0..8 {
                events_tx.send(Event::Reload).await.unwrap();
            }
            assert!(
                get(address, "/v1/service/ready")
                    .await
                    .starts_with("HTTP/1.1 200")
            );
            events_tx.send(Event::Stop).await.unwrap();
        };
        let (report, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(running, exercise)
        })
        .await
        .unwrap();
        report
    });
    // The app returned without waiting forever; now release our intentional
    // blocking test worker before destroying the test runtime/temporary files.
    drop(release);
    runtime.shutdown(context.runtime_deadline(cfg.boot.shutdown.runtime));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(report.unreaped.contains(&crate::config::reload::JOB_NAME));
    assert!(report.forced && report.cleanup_failed && !report.succeeded());
    assert_eq!(report.storage, StorageClose::SkippedUnproven);
}

#[tokio::test]
async fn a_loader_panic_triggers_process_level_shutdown_instead_of_a_reload_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let mut context = ShutdownContext::default();
    let (tx, mut rx) = mpsc::channel(2);
    let (ready_tx, ready_rx) = oneshot::channel();
    let running = run_with_loader(
        &cfg,
        build(),
        &mut context,
        async || rx.recv().await.ok_or(ControlClosed),
        |address| ready_tx.send(address).unwrap(),
        || panic!("controlled reload panic"),
    );
    let trigger = async {
        ready_rx.await.unwrap();
        tx.send(Event::Reload).await.unwrap();
    };
    let (report, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(running, trigger)
    })
    .await
    .unwrap();
    assert!(matches!(
        report.cause,
        StopCause::ReloadFailure("loader_panic")
    ));
    assert_eq!(report.storage, StorageClose::Closed);
    assert!(report.unreaped.is_empty());
    assert!(!report.succeeded());
}

#[tokio::test]
async fn startup_reload_requests_do_not_create_a_background_job() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let loaded = cfg.clone();
    let mut context = ShutdownContext::default();
    let (tx, mut rx) = mpsc::channel(2);
    tx.send(Event::Reload).await.unwrap();
    tx.send(Event::Stop).await.unwrap();
    let report = run_with_loader(
        &cfg,
        build(),
        &mut context,
        async || rx.recv().await.ok_or(ControlClosed),
        |_| panic!("committed startup"),
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(loaded.clone())
        },
    )
    .await;
    assert!(report.succeeded());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_queued_stop_wins_over_a_reload_completion_and_cannot_publish_after_shutdown() {
    let mut context = ShutdownContext::default();
    let mut resources = fake_resources(&mut context);
    let dir = tempfile::tempdir().unwrap();
    let initial = config(dir.path());
    let mut fresh = initial.clone();
    fresh.hot.ticker = service_core::TickerInterval::try_from(Duration::from_millis(50)).unwrap();
    resources.reload = ReloadController::new(&initial, move || Ok(fresh.clone()));
    let reader = resources.reload.reader();
    let child = resources.gate.root.child_token();
    resources
        .supervisor
        .spawn_on("http", &Handle::current(), "main", async move {
            child.cancelled().await;
            Ok(())
        })
        .unwrap();
    resources.reload.request();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(Event::Stop).await.unwrap();
    monitor(&mut resources, &mut async || {
        rx.recv().await.ok_or(ControlClosed)
    })
    .await;
    let report = cleanup(&mut resources, &mut async || {
        rx.recv().await.ok_or(ControlClosed)
    })
    .await;
    resources.finished = true;
    assert!(matches!(report.cause, StopCause::Signal));
    assert_eq!(reader.current().generation, 1);
    assert_eq!(reader.current().config, initial.hot);
}

#[tokio::test]
async fn finishing_an_inflight_load_during_grace_is_discarded_without_forcing_shutdown() {
    use std::sync::{Arc, Condvar, Mutex};
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(gate.clone());
    let (started_tx, started_rx) = oneshot::channel();
    let started_tx = Mutex::new(Some(started_tx));
    let next = cfg.clone();
    let loader = move || {
        started_tx.lock().unwrap().take().unwrap().send(()).unwrap();
        let (lock, wake) = &*gate;
        let mut allowed = lock.lock().unwrap();
        while !*allowed {
            allowed = wake.wait(allowed).unwrap();
        }
        Ok(next.clone())
    };
    let mut context = ShutdownContext::default();
    let (tx, mut rx) = mpsc::channel(4);
    let (ready_tx, ready_rx) = oneshot::channel();
    let running = run_with_loader(
        &cfg,
        build(),
        &mut context,
        async || rx.recv().await.ok_or(ControlClosed),
        |address| ready_tx.send(address).unwrap(),
        loader,
    );
    let trigger = async {
        ready_rx.await.unwrap();
        tx.send(Event::Reload).await.unwrap();
        started_rx.await.unwrap();
        tx.send(Event::Stop).await.unwrap();
        drop(release);
    };
    let (report, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(running, trigger)
    })
    .await
    .unwrap();
    assert!(report.succeeded());
    assert!(!report.forced);
    assert_eq!(report.storage, StorageClose::Closed);
    assert!(report.unreaped.is_empty());
}

#[test]
fn a_saturated_extra_worker_does_not_stall_main_http_or_its_shared_storage_probe() {
    use crate::config::{Binding, RuntimeSpec};
    use std::sync::{Arc, Condvar, Mutex};
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path());
    cfg.boot.ticker_runtime = Binding::Extra("compute".into());
    cfg.boot.extra_runtimes.insert(
        "compute".into(),
        RuntimeSpec {
            worker_threads: 1,
            max_blocking_threads: 1,
        },
    );
    let mut runtimes = crate::rt::RuntimeSet::build(&cfg.boot).unwrap();
    let executors = runtimes.executors();
    let blocked = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(blocked.clone());
    let mut context = ShutdownContext::default();
    let snapshot = cfg.clone();
    let report = runtimes.block_on(async {
        let (events_tx, mut events_rx) = mpsc::channel(2);
        let (ready_tx, ready_rx) = oneshot::channel();
        let running = super::run(
            &cfg,
            build(),
            &mut context,
            async || events_rx.recv().await.ok_or(ControlClosed),
            |address| ready_tx.send(address).unwrap(),
            move || Ok(snapshot.clone()),
            &executors,
        );
        let exercise = async {
            let address = ready_rx.await.unwrap();
            let (started_tx, started_rx) = oneshot::channel();
            let blocker = executors
                .resolve(&cfg.boot.ticker_runtime)
                .unwrap()
                .handle
                .spawn(async move {
                    started_tx.send(()).unwrap();
                    // Test-only scheduler starvation: keep the only extra worker
                    // occupied after the real ticker has confirmed startup.
                    let (lock, wake) = &*blocked;
                    let mut allowed = lock.lock().unwrap();
                    while !*allowed {
                        allowed = wake.wait(allowed).unwrap();
                    }
                });
            started_rx.await.unwrap();
            let response =
                tokio::time::timeout(Duration::from_secs(2), get(address, "/v1/service/ready"))
                    .await
                    .unwrap();
            assert!(response.starts_with("HTTP/1.1 200"));
            drop(release);
            blocker.await.unwrap();
            events_tx.send(Event::Stop).await.unwrap();
        };
        let (report, ()) = tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(running, exercise)
        })
        .await
        .unwrap();
        report
    });
    assert!(report.succeeded());
    assert_eq!(report.storage, StorageClose::Closed);
    assert!(
        report
            .tasks
            .iter()
            .any(|task| task.name == "ticker" && task.runtime == "compute")
    );
    assert!(
        report
            .tasks
            .iter()
            .any(|task| task.name == "http" && task.runtime == "main")
    );
    assert!(!runtimes.shutdown(context.runtime_deadline(cfg.boot.shutdown.runtime)));
}
