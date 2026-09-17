//! supervisor 行为测试：五类退出、单化身、重启边界、空 supervisor、runtime 归属。
//!
//! 全部用 `start_paused = true` 的虚拟时钟：预算与退避都是确定性的，测试不靠墙钟。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use {{crate_prefix_snake}}_runtime::{
    Backoff, Error, ExitCause, ExitRecord, RestartPolicy, RuntimeId, RuntimeSet, ShutdownBudget,
    ShutdownReport, StopSignal, StopTrigger, Supervisor, TaskContext, TaskKey, TaskOutcome,
    TaskSpec, shared_backoff,
};
use tokio::runtime::{Builder, Handle, Runtime};
use tokio::task::JoinHandle;
use tokio::time::Instant;

fn budget() -> ShutdownBudget {
    ShutdownBudget {
        total: Duration::from_secs(10),
        drain: Duration::from_millis(50),
        harvest: Duration::from_millis(200),
        reap: Duration::from_millis(50),
        resources: Duration::from_millis(100),
        forced_phase: Duration::from_millis(10),
    }
}

fn build(specs: Vec<TaskSpec>, aux: Option<(RuntimeId, &Runtime)>) -> (Supervisor, StopSignal) {
    let mut runtimes = RuntimeSet::new(Handle::current());
    if let Some((id, runtime)) = aux {
        runtimes = runtimes.with_aux(id, runtime.handle().clone());
    }
    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        runtimes,
        budget(),
        shared_backoff(Backoff::default()),
        signal.clone(),
    );
    for spec in specs {
        supervisor.register(spec).expect("注册应当成功");
    }
    supervisor.start().expect("启动应当成功");
    (supervisor, signal)
}

/// 把 supervisor 放到一个 task 里跑（`run` 需要 `'static`）。
fn spawn_run(supervisor: Supervisor, signal: &StopSignal) -> JoinHandle<ShutdownReport> {
    let signal = signal.clone();
    tokio::spawn(async move {
        let signal = signal.clone();
        supervisor.run(&signal).await
    })
}

async fn stop_and_collect(supervisor: Supervisor, signal: &StopSignal) -> ShutdownReport {
    let runner = spawn_run(supervisor, signal);
    signal.request_drain();
    runner.await.expect("supervisor 任务不应 panic")
}

fn record_for<'a>(report: &'a ShutdownReport, key: &str) -> &'a ExitRecord {
    report
        .records
        .iter()
        .find(|record| record.key.as_str() == key)
        .unwrap_or_else(|| panic!("没有 key={key} 的退出记录：{:?}", report.records))
}

#[tokio::test(start_paused = true)]
async fn records_completed_exit_for_task_returning_ok() {
    let spec = TaskSpec::new(TaskKey::new("one"), RuntimeId::MAIN, |_| {
        Box::pin(async { Ok(()) })
    })
    .fatal(false);
    let (supervisor, signal) = build(vec![spec], None);

    let runner = spawn_run(supervisor, &signal);
    tokio::time::sleep(Duration::from_millis(5)).await; // 让任务先结束
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    let record = record_for(&report, "one");
    assert_eq!(record.cause, ExitCause::Completed);
    assert_eq!(record.outcome, TaskOutcome::Returned);
    assert_eq!(record.runtime, RuntimeId::MAIN);
    assert_eq!(record.incarnation, 1);
}

#[tokio::test(start_paused = true)]
async fn records_failed_exit_for_task_returning_err() {
    let spec = TaskSpec::new(TaskKey::new("one"), RuntimeId::MAIN, |_| {
        Box::pin(async { Err(Error::task("boom")) })
    })
    .fatal(false);
    let (supervisor, signal) = build(vec![spec], None);
    let report = stop_and_collect(supervisor, &signal).await;

    let record = record_for(&report, "one");
    assert_eq!(record.cause, ExitCause::Failed);
    assert!(matches!(&record.outcome, TaskOutcome::Failed { message } if message.contains("boom")));
}

#[tokio::test(start_paused = true)]
async fn records_panicked_exit_for_task_panicking() {
    let spec = TaskSpec::new(TaskKey::new("crasher"), RuntimeId::MAIN, |_| {
        Box::pin(async {
            panic!("任务炸了");
            #[allow(unreachable_code)]
            Ok(())
        })
    })
    .fatal(false);
    let (supervisor, signal) = build(vec![spec], None);
    let report = stop_and_collect(supervisor, &signal).await;

    let record = record_for(&report, "crasher");
    assert_eq!(record.cause, ExitCause::Panicked);
    assert!(
        matches!(&record.outcome, TaskOutcome::Panicked { message } if message.contains("任务炸了"))
    );
}

#[tokio::test(start_paused = true)]
async fn records_cancelled_exit_for_task_cancelled_at_shutdown() {
    let spec = TaskSpec::new(
        TaskKey::new("waiter"),
        RuntimeId::MAIN,
        |ctx: TaskContext| {
            Box::pin(async move {
                ctx.cancelled().await;
                Ok(())
            })
        },
    );
    let (supervisor, signal) = build(vec![spec], None);
    let report = stop_and_collect(supervisor, &signal).await;

    let record = record_for(&report, "waiter");
    assert_eq!(record.cause, ExitCause::Cancelled, "关停期退出算 Cancelled");
    assert_eq!(
        record.outcome,
        TaskOutcome::Returned,
        "优雅返回的事实要留着"
    );
    assert_eq!(report.stopped, vec![TaskKey::new("waiter")]);
    assert!(report.clean(), "{report:?}");
}

#[tokio::test(start_paused = true)]
async fn records_restarted_exit_when_restart_is_requested() {
    let spec = TaskSpec::new(
        TaskKey::new("worker"),
        RuntimeId::MAIN,
        |ctx: TaskContext| {
            Box::pin(async move {
                ctx.cancelled().await;
                Ok(())
            })
        },
    );
    let (supervisor, signal) = build(vec![spec], None);
    let handle = supervisor.handle();
    let runner = spawn_run(supervisor, &signal);

    handle
        .restart(&TaskKey::new("worker"))
        .await
        .expect("第一次重启应当成功");
    tokio::time::sleep(Duration::from_millis(5)).await;
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    let restarted: Vec<&ExitRecord> = report
        .records
        .iter()
        .filter(|record| record.key.as_str() == "worker")
        .collect();
    assert_eq!(restarted.len(), 2, "{:?}", report.records);
    assert_eq!(restarted[0].cause, ExitCause::Restarted);
    assert_eq!(restarted[0].incarnation, 1);
    assert_eq!(restarted[1].cause, ExitCause::Cancelled);
    assert_eq!(restarted[1].incarnation, 2);
}

#[tokio::test(start_paused = true)]
async fn restart_waits_for_previous_incarnation_single_identity() {
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (live_task, peak_task) = (live.clone(), peak.clone());
    let spec = TaskSpec::new(
        TaskKey::new("worker"),
        RuntimeId::MAIN,
        move |ctx: TaskContext| {
            let live = live_task.clone();
            let peak = peak_task.clone();
            Box::pin(async move {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                ctx.cancelled().await;
                live.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        },
    );
    let (supervisor, signal) = build(vec![spec], None);
    let handle = supervisor.handle();
    let runner = spawn_run(supervisor, &signal);

    for _ in 0..3 {
        handle.restart(&TaskKey::new("worker")).await.expect("重启");
    }
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "任何时刻同一 key 只能有一个化身"
    );
    assert_eq!(live.load(Ordering::SeqCst), 0, "关停后不该有活着的化身");
    assert_eq!(
        report
            .records
            .iter()
            .filter(|record| record.key.as_str() == "worker")
            .count(),
        4,
        "初始 + 三次替换"
    );
}

#[tokio::test(start_paused = true)]
async fn restart_is_refused_if_previous_incarnation_will_not_stop() {
    let spec = TaskSpec::new(TaskKey::new("stubborn"), RuntimeId::MAIN, |_| {
        Box::pin(std::future::pending::<Result<(), Error>>())
    });
    let (supervisor, signal) = build(vec![spec], None);
    let handle = supervisor.handle();
    let runner = spawn_run(supervisor, &signal);

    let err = handle
        .restart(&TaskKey::new("stubborn"))
        .await
        .expect_err("旧化身不结束时必须拒绝重启");
    assert!(err.to_string().contains("没有结束"), "{err}");

    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");
    assert_eq!(
        report.aborted,
        vec![TaskKey::new("stubborn")],
        "不肯走的会被 abort"
    );
    assert_eq!(
        report
            .records
            .iter()
            .filter(|record| record.key.as_str() == "stubborn")
            .count(),
        1,
        "被拒绝的重启不能留下第二个化身：{:?}",
        report.records
    );
    // "abort 也收不回来"（still_running 的真实形态）在 tests/shutdown.rs 里用同步阻塞任务覆盖：
    // 虚拟时钟下的 pending() 一旦被 abort 就立即结束，构造不出 still_running。
}

#[tokio::test(start_paused = true)]
async fn empty_supervisor_stops_cleanly() {
    let (supervisor, signal) = build(vec![], None);
    let report = stop_and_collect(supervisor, &signal).await;
    assert!(report.clean(), "{report:?}");
    assert!(report.stopped.is_empty());
    assert!(report.records.is_empty());
}

#[tokio::test(start_paused = true)]
async fn no_restart_happens_after_stop_requested() {
    let spec = TaskSpec::new(TaskKey::new("flaky"), RuntimeId::MAIN, |_| {
        Box::pin(async { Err(Error::task("always fails")) })
    })
    .fatal(false)
    .restart(RestartPolicy::restart(5));
    let (supervisor, signal) = build(vec![spec], None);
    let runner = spawn_run(supervisor, &signal);

    // 第一次失败 → 安排重启（退避 500ms）；在退避到期之前就停止。
    tokio::time::sleep(Duration::from_millis(10)).await;
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    assert_eq!(
        report.records.len(),
        1,
        "停止之后不得再起新化身：{:?}",
        report.records
    );
    assert_eq!(report.records[0].incarnation, 1);
    assert_eq!(report.records[0].cause, ExitCause::Failed);
}

#[tokio::test(start_paused = true)]
async fn fatal_exit_requests_stop_with_task_failure_trigger() {
    let spec = TaskSpec::new(TaskKey::new("critical"), RuntimeId::MAIN, |_| {
        Box::pin(async { Err(Error::task("critical failed")) })
    });
    let (supervisor, signal) = build(vec![spec], None);
    // 不需要任何停止信号：fatal 任务自行退出就应当触发关停。
    let report = supervisor.run(&signal).await;

    assert_eq!(
        report.trigger,
        StopTrigger::TaskFailure {
            key: TaskKey::new("critical")
        }
    );
    assert_eq!(record_for(&report, "critical").cause, ExitCause::Failed);
    assert!(report.clean(), "触发原因不影响关停本身要干净：{report:?}");
}

#[tokio::test(start_paused = true)]
async fn restart_budget_exhaustion_is_terminal_and_fatal() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    let spec = TaskSpec::new(TaskKey::new("flaky"), RuntimeId::MAIN, move |_| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(Error::task("nope"))
        })
    })
    .restart(RestartPolicy::restart(2));
    let (supervisor, signal) = build(vec![spec], None);
    let report = supervisor.run(&signal).await;

    assert_eq!(attempts.load(Ordering::SeqCst), 3, "初始 + 两次重启");
    assert_eq!(report.records.len(), 3);
    assert!(matches!(report.trigger, StopTrigger::TaskFailure { .. }));
}

#[tokio::test(start_paused = true)]
async fn restart_backoff_is_exponential_and_capped() {
    let starts: Arc<std::sync::Mutex<Vec<Instant>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = starts.clone();
    let spec = TaskSpec::new(TaskKey::new("flaky"), RuntimeId::MAIN, move |_| {
        let recorder = recorder.clone();
        Box::pin(async move {
            recorder.lock().expect("锁").push(Instant::now());
            Err(Error::task("nope"))
        })
    })
    .fatal(false)
    .restart(RestartPolicy::restart(3));

    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        RuntimeSet::new(Handle::current()),
        budget(),
        shared_backoff(Backoff {
            base: Duration::from_millis(100),
            cap: Duration::from_millis(250),
        }),
        signal.clone(),
    );
    supervisor.register(spec).expect("注册");
    supervisor.start().expect("启动");
    let runner = spawn_run(supervisor, &signal);

    // 等 4 次尝试（初始 + 3 次重启）都跑完：退避 100ms、200ms、250ms（封顶）
    tokio::time::sleep(Duration::from_millis(700)).await;
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");
    assert_eq!(report.records.len(), 4, "{:?}", report.records);

    let starts = starts.lock().expect("锁").clone();
    assert_eq!(starts.len(), 4, "{starts:?}");
    let gaps: Vec<Duration> = starts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(gaps[0], Duration::from_millis(100), "{gaps:?}");
    assert_eq!(gaps[1], Duration::from_millis(200), "{gaps:?}");
    assert_eq!(gaps[2], Duration::from_millis(250), "封顶：{gaps:?}");
}

// 这个用例不能用虚拟时钟：任务跑在另一个（真实时钟的）runtime 上，暂停时钟无法同步跨 runtime 的唤醒。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_spawned_to_aux_runtime_is_harvested_with_its_runtime_identity() {
    let aux = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("aux runtime");
    let spec = TaskSpec::new(
        TaskKey::new("io-work"),
        RuntimeId::new("io"),
        |ctx: TaskContext| {
            Box::pin(async move {
                ctx.cancelled().await;
                Ok(())
            })
        },
    );
    let (supervisor, signal) = build(vec![spec], Some((RuntimeId::new("io"), &aux)));
    let report = stop_and_collect(supervisor, &signal).await;

    let record = record_for(&report, "io-work");
    assert_eq!(record.runtime, RuntimeId::new("io"));
    assert_eq!(record.cause, ExitCause::Cancelled);
    assert_eq!(report.stopped, vec![TaskKey::new("io-work")]);

    // 附加 runtime 不能在 async 上下文里 drop（tokio 会 panic），必须先显式后台关停。
    aux.shutdown_background();
}

#[tokio::test(start_paused = true)]
async fn registering_unknown_runtime_is_a_startup_error() {
    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        RuntimeSet::new(Handle::current()),
        budget(),
        shared_backoff(Backoff::default()),
        signal,
    );
    supervisor
        .register(TaskSpec::new(
            TaskKey::new("x"),
            RuntimeId::new("nope"),
            |_| Box::pin(async { Ok(()) }),
        ))
        .expect("注册允许，校验在启动时");
    let err = supervisor
        .start()
        .expect_err("未注册的 runtime 必须拒绝启动");
    assert!(err.to_string().contains("nope"), "{err}");
    assert!(
        err.to_string().contains("main"),
        "错误里要给出已注册的 runtime：{err}"
    );
}

#[tokio::test(start_paused = true)]
async fn duplicate_key_registration_is_rejected() {
    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        RuntimeSet::new(Handle::current()),
        budget(),
        shared_backoff(Backoff::default()),
        signal,
    );
    supervisor
        .register(TaskSpec::new(TaskKey::new("same"), RuntimeId::MAIN, |_| {
            Box::pin(async { Ok(()) })
        }))
        .expect("第一次注册");
    let err = supervisor
        .register(TaskSpec::new(TaskKey::new("same"), RuntimeId::MAIN, |_| {
            Box::pin(async { Ok(()) })
        }))
        .expect_err("重复 key 必须拒绝");
    assert!(err.to_string().contains("重复"), "{err}");
}

#[tokio::test(start_paused = true)]
async fn tracked_tasks_all_finished_after_shutdown() {
    let live = Arc::new(AtomicUsize::new(0));
    let counter = live.clone();
    let spec = TaskSpec::new(
        TaskKey::new("worker"),
        RuntimeId::MAIN,
        move |ctx: TaskContext| {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                ctx.cancelled().await;
                counter.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        },
    )
    .restart(RestartPolicy::restart(5));
    let (supervisor, signal) = build(vec![spec], None);
    let report = stop_and_collect(supervisor, &signal).await;

    assert_eq!(live.load(Ordering::SeqCst), 0, "不留 detached / 不留活化身");
    assert!(report.still_running.is_empty());
    assert!(report.clean(), "{report:?}");
}

#[tokio::test(start_paused = true)]
async fn exit_records_are_published_to_subscribers() {
    let spec = TaskSpec::new(TaskKey::new("one"), RuntimeId::MAIN, |ctx: TaskContext| {
        Box::pin(async move {
            ctx.cancelled().await;
            Ok(())
        })
    });
    let (supervisor, signal) = build(vec![spec], None);
    let mut events = supervisor.subscribe();
    let report = stop_and_collect(supervisor, &signal).await;

    let mut seen = Vec::new();
    while let Ok(record) = events.try_recv() {
        seen.push(record);
    }
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].cause, ExitCause::Cancelled);
    assert_eq!(seen[0].key, TaskKey::new("one"));
    assert_eq!(report.records.len(), 1, "报告与通道必须是同一批记录");
}
