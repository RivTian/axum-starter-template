//! 生命周期与退出码：看门狗、构建串、干净关停（每个任务都 stopped）、启动失败。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use {{crate_prefix_snake}}_app::{Args, Assembly, ProcessExit, Telemetry, exit_code, resolve_with_anchor, watchdog};
use {{crate_prefix_snake}}_config::{Anchor, EnvSource, ErrorKind, MapEnv};
use {{crate_prefix_snake}}_runtime::{ShutdownReport, StopSignal, StopTrigger, TaskKey};
use tempfile::TempDir;
use tokio::runtime::Builder;

#[test]
fn exit_code_mapping_covers_all_cases() {
    let clean = ShutdownReport::new(StopTrigger::Signal);
    assert_eq!(exit_code(&clean), ProcessExit::Clean);
    assert_eq!(ProcessExit::Clean.code(), 0);

    let mut forced = ShutdownReport::new(StopTrigger::Signal);
    forced.forced = true;
    assert_eq!(exit_code(&forced), ProcessExit::IncompleteShutdown);
    assert_eq!(ProcessExit::IncompleteShutdown.code(), 4);

    let failed = ShutdownReport::new(StopTrigger::TaskFailure {
        key: TaskKey::new("x"),
    });
    assert_eq!(exit_code(&failed), ProcessExit::TaskFailure);
    assert_eq!(ProcessExit::TaskFailure.code(), 3);

    let mut incomplete = ShutdownReport::new(StopTrigger::Signal);
    incomplete.still_running.push(TaskKey::new("y"));
    assert_eq!(exit_code(&incomplete), ProcessExit::IncompleteShutdown);

    let mut resource_failure = ShutdownReport::new(StopTrigger::Signal);
    resource_failure
        .resource_failures
        .push("storage: 关闭超时".to_owned());
    assert_eq!(
        exit_code(&resource_failure),
        ProcessExit::IncompleteShutdown
    );
}

#[test]
fn build_string_answers_which_code_is_running() {
    let rendered = {{crate_prefix_snake}}_app::build_string();
    assert!(rendered.contains(env!("CARGO_PKG_NAME")), "{rendered}");
    assert!(rendered.contains(env!("CARGO_PKG_VERSION")), "{rendered}");
    assert!(rendered.contains("git="), "{rendered}");
    assert!(rendered.contains("dirty="), "{rendered}");
    assert!(rendered.contains("profile="), "{rendered}");
}

#[tokio::test(start_paused = true)]
async fn watchdog_fires_at_the_deadline() {
    let fired = Arc::new(AtomicBool::new(false));
    let flag = fired.clone();
    let task = tokio::spawn(watchdog(Duration::from_secs(15), move || {
        flag.store(true, Ordering::SeqCst);
    }));

    tokio::time::sleep(Duration::from_secs(14)).await;
    assert!(!fired.load(Ordering::SeqCst), "预算内不该触发");
    tokio::time::sleep(Duration::from_secs(2)).await;
    task.await.expect("watchdog 任务");
    assert!(fired.load(Ordering::SeqCst), "到点必须触发");
}

#[tokio::test(start_paused = true)]
async fn watchdog_is_silent_when_aborted() {
    let fired = Arc::new(AtomicBool::new(false));
    let flag = fired.clone();
    let task = tokio::spawn(watchdog(Duration::from_secs(15), move || {
        flag.store(true, Ordering::SeqCst);
    }));
    tokio::time::sleep(Duration::from_secs(1)).await;
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(!fired.load(Ordering::SeqCst), "被 abort 之后不该再触发");
}

/// DoD 3 的进程内版本：装配跑起来、收到停止请求后每个任务都在报告里 stopped、退出码干净。
///
/// 用同步 `#[test]`：这里要自己建 runtime，而 tokio 的 Runtime 不能在 async 上下文里 drop。
#[test]
fn clean_stop_reports_every_task_stopped() {
    let temp = TempDir::new().expect("临时目录");
    let env: Arc<dyn EnvSource> = Arc::new(MapEnv::new());
    let telemetry = Arc::new(Telemetry::new("info").expect("日志过滤器"));
    let settings = resolve_with_anchor(&Args { config: None }, &env, Anchor::from_dir(temp.path()))
        .expect("设置解析");
    let assembly = Assembly::prepare(settings, env, telemetry).expect("装配");

    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let stop = StopSignal::new();

    let outcome = runtime.block_on(async {
        let stopper = tokio::spawn({
            let stop = stop.clone();
            async move {
                // 给任务一点时间真正跑起来（监听绑定、watcher 就绪）。
                tokio::time::sleep(Duration::from_millis(300)).await;
                stop.request_drain();
            }
        });
        let outcome = assembly.run(&runtime, stop).await;
        let _ = stopper.await;
        outcome
    });

    let outcome = outcome.expect("装配应当跑到关停");
    let stopped: Vec<&str> = outcome.report.stopped.iter().map(TaskKey::as_str).collect();
    assert!(
        stopped.contains(&"http"),
        "每个任务都要 stopped：{stopped:?}"
    );
    assert!(
        stopped.contains(&"config-watch"),
        "每个任务都要 stopped：{stopped:?}"
    );
    assert!(outcome.report.clean(), "{:?}", outcome.report);
    assert_eq!(exit_code(&outcome.report), ProcessExit::Clean);
    assert!(
        outcome
            .report
            .resources_closed
            .contains(&"storage".to_owned()),
        "存储必须在任务收尾之后关闭：{:?}",
        outcome.report
    );

    runtime.shutdown_timeout(Duration::from_secs(1));
}

/// 启动失败（监听绑不上）在提交点之前就返回，且分类是 Startup。
///
/// 用 TEST-NET-1 的地址（192.0.2.0/24）：语法合法、配置校验会放行，但绑不上——正是"必然失败"的形态。
#[test]
fn startup_failure_is_reported_before_any_task_runs() {
    let temp = TempDir::new().expect("临时目录");
    let env: Arc<dyn EnvSource> = Arc::new(MapEnv::new());
    let telemetry = Arc::new(Telemetry::new("info").expect("日志过滤器"));

    std::fs::write(
        temp.path().join("config.toml"),
        "[http]\nbind = \"192.0.2.1:0\"\n",
    )
    .expect("写配置");

    let settings = resolve_with_anchor(&Args { config: None }, &env, Anchor::from_dir(temp.path()))
        .expect("设置解析（地址语法合法）");
    let assembly = Assembly::prepare(settings, env, telemetry).expect("装配");
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let error = runtime
        .block_on(assembly.run(&runtime, StopSignal::new()))
        .err()
        .expect("绑不上监听必须拒启动");
    assert_eq!(error.kind(), ErrorKind::Startup);
    assert!(error.to_string().contains("绑定"), "{error}");
    runtime.shutdown_timeout(Duration::from_secs(1));
}
