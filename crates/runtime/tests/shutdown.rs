//! 关停序列测试：分层预算、二次信号加速、诚实上报、资源逆序关闭。
//!
//! 预算类用例用虚拟时钟（确定性）；"abort 之后仍不回来"的用例需要真实时钟 + 多线程 runtime，
//! 因为它靠"同步阻塞、不在 await 点上"来构造——这正是 abort 收不回来的真实形态。

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use {{crate_prefix_snake}}_runtime::{
    Backoff, Error, ExitCause, RuntimeId, RuntimeSet, ShutdownBudget, ShutdownReport, StopSignal,
    Supervisor, TaskContext, TaskKey, TaskSpec, shared_backoff,
};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::Instant;

fn budget(drain: u64, harvest: u64, reap: u64, resources: u64) -> ShutdownBudget {
    ShutdownBudget {
        total: Duration::from_secs(30),
        drain: Duration::from_millis(drain),
        harvest: Duration::from_millis(harvest),
        reap: Duration::from_millis(reap),
        resources: Duration::from_millis(resources),
        forced_phase: Duration::from_millis(10),
    }
}

fn supervisor_with(specs: Vec<TaskSpec>, budget: ShutdownBudget) -> (Supervisor, StopSignal) {
    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        RuntimeSet::new(Handle::current()),
        budget,
        shared_backoff(Backoff::default()),
        signal.clone(),
    );
    for spec in specs {
        supervisor.register(spec).expect("注册");
    }
    supervisor.start().expect("启动");
    (supervisor, signal)
}

fn spawn_run(supervisor: Supervisor, signal: &StopSignal) -> JoinHandle<ShutdownReport> {
    let signal = signal.clone();
    tokio::spawn(async move {
        let signal = signal.clone();
        supervisor.run(&signal).await
    })
}

fn waiter(key: &str) -> TaskSpec {
    let key = TaskKey::new(key);
    TaskSpec::new(key, RuntimeId::MAIN, |ctx: TaskContext| {
        Box::pin(async move {
            ctx.cancelled().await;
            Ok(())
        })
    })
}

#[tokio::test(start_paused = true)]
async fn every_phase_respects_absolute_budget() {
    // 一个永远不结束的任务（虚拟时钟下就是 pending）会把 harvest 与 reap 都用满。
    let stuck = TaskSpec::new(TaskKey::new("stuck"), RuntimeId::MAIN, |_| {
        Box::pin(std::future::pending::<Result<(), Error>>())
    });
    let budget = budget(100, 200, 50, 100);
    let (supervisor, signal) = supervisor_with(vec![stuck], budget);
    let runner = spawn_run(supervisor, &signal);
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    // drain 100 + harvest 200 是必等满的两段；abort 之后 pending 任务立刻结束，reap 不消耗预算。
    // 断言"每段都不超过自己的上限、总和不超过各段之和"。
    assert!(
        report.elapsed >= Duration::from_millis(300) && report.elapsed <= budget.inner_sum(),
        "关停耗时应落在 [drain+harvest, 各段之和] 内：{:?}",
        report.elapsed
    );
    assert!(budget.inner_sum() < budget.total);
    assert!(report.elapsed < budget.total);
    assert_eq!(report.aborted, vec![TaskKey::new("stuck")]);
}

#[tokio::test(start_paused = true)]
async fn second_stop_request_accelerates_without_extending_deadline() {
    let stuck = TaskSpec::new(TaskKey::new("stuck"), RuntimeId::MAIN, |_| {
        Box::pin(std::future::pending::<Result<(), Error>>())
    });
    let (supervisor, signal) = supervisor_with(vec![stuck], budget(2_000, 4_000, 500, 2_000));
    let runner = spawn_run(supervisor, &signal);

    let requested = Instant::now();
    signal.request_drain();
    signal.force(); // 立刻来第二次：只加速
    let report = runner.await.expect("supervisor 任务");

    assert!(report.forced, "报告必须标注被强推过");
    assert!(
        report.elapsed < Duration::from_millis(500),
        "二次信号之后不该再等满 drain+harvest：{:?}",
        report.elapsed
    );
    let wall = requested.elapsed();
    assert!(wall < Duration::from_millis(500), "{wall:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reports_still_running_task_honestly() {
    // 同步阻塞的任务不在 await 点上：abort 收不回来——这就是"收不回来"的真实形态。
    let (release, blocked) = channel::<()>();
    let blocked = Arc::new(Mutex::new(blocked));
    let stuck = TaskSpec::new(TaskKey::new("stuck"), RuntimeId::MAIN, move |_| {
        let blocked = blocked.clone();
        Box::pin(async move {
            let guard = blocked.lock().expect("锁");
            let _ = guard.recv();
            Ok(())
        })
    });
    let (supervisor, signal) = supervisor_with(vec![stuck], budget(50, 100, 50, 100));
    let runner = spawn_run(supervisor, &signal);
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    assert!(
        report.still_running.contains(&TaskKey::new("stuck")),
        "收不回来的任务必须如实上报为 still_running：{report:?}"
    );
    assert_eq!(report.aborted, vec![TaskKey::new("stuck")]);
    assert!(!report.clean(), "有未收割任务时不算干净关停");
    assert!(
        !report.stopped.contains(&TaskKey::new("stuck")),
        "没收尾的任务不许进 stopped"
    );

    // 放手，让被阻塞的线程走完，避免测试 runtime 关闭时干等。
    let _ = release.send(());
}

#[tokio::test(start_paused = true)]
async fn resources_close_in_reverse_registration_order() {
    let closed: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (first, second) = (closed.clone(), closed.clone());
    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        RuntimeSet::new(Handle::current()),
        budget(10, 50, 10, 100),
        shared_backoff(Backoff::default()),
        signal.clone(),
    );
    supervisor
        .register_resource("first", async move {
            first.lock().expect("锁").push("first");
            Ok(())
        })
        .expect("注册资源");
    supervisor
        .register_resource("second", async move {
            second.lock().expect("锁").push("second");
            Ok(())
        })
        .expect("注册资源");
    supervisor.register(waiter("task")).expect("注册任务");
    supervisor.start().expect("启动");

    let runner = spawn_run(supervisor, &signal);
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    assert_eq!(
        closed.lock().expect("锁").clone(),
        vec!["second", "first"],
        "资源必须逆注册序关闭（最后打开的最先关）"
    );
    assert_eq!(report.resources_closed, vec!["second", "first"]);
    assert!(report.clean(), "{report:?}");
}

#[tokio::test(start_paused = true)]
async fn resource_close_is_bounded_and_reported_honestly() {
    let signal = StopSignal::new();
    let mut supervisor = Supervisor::new(
        RuntimeSet::new(Handle::current()),
        budget(10, 50, 10, 100),
        shared_backoff(Backoff::default()),
        signal.clone(),
    );
    supervisor
        .register_resource("slow", std::future::pending::<Result<(), Error>>())
        .expect("注册资源");
    supervisor.start().expect("启动");

    let runner = spawn_run(supervisor, &signal);
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    assert!(
        report
            .resource_failures
            .iter()
            .any(|failure| failure.contains("slow") && failure.contains("超时")),
        "资源关闭超时必须如实上报：{report:?}"
    );
    assert!(!report.clean());
}

#[tokio::test(start_paused = true)]
async fn tasks_that_finished_before_stop_are_not_counted_as_stopped() {
    let quick = TaskSpec::new(TaskKey::new("quick"), RuntimeId::MAIN, |_| {
        Box::pin(async { Ok(()) })
    })
    .fatal(false);
    let (supervisor, signal) = supervisor_with(vec![quick], budget(50, 100, 50, 100));
    let runner = spawn_run(supervisor, &signal);

    // 等它结束（记录写着 Completed），然后再停止。
    tokio::time::sleep(Duration::from_millis(10)).await;
    signal.request_drain();
    let report = runner.await.expect("supervisor 任务");

    assert_eq!(report.records[0].cause, ExitCause::Completed);
    assert!(
        report.stopped.is_empty(),
        "停止之前就结束的任务不该出现在 stopped 里：{report:?}"
    );
    assert!(report.clean(), "{report:?}");
}
