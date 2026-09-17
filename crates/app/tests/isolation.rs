//! 零全局态的证据：一个进程里同时跑两套独立装配（生命周期、配置、数据库、取消互不干扰）。
//!
//! 两套装配各自：有自己的锚点（配置文件 + 数据库都在自己锚点下）、自己的停止信号、自己的
//! `Telemetry`（只构造不安装）、自己的配置视图。A 停了 B 照常工作，配置重载也只影响自己。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::runtime::Builder;
use tokio::sync::watch;

use {{crate_prefix_snake}}_app::Args;
use {{crate_prefix_snake}}_app::Assembly;
use {{crate_prefix_snake}}_app::ProcessExit;
use {{crate_prefix_snake}}_app::Telemetry;
use {{crate_prefix_snake}}_app::exit_code;
use {{crate_prefix_snake}}_app::resolve_with_anchor;
use {{crate_prefix_snake}}_config::Anchor;
use {{crate_prefix_snake}}_config::Config;
use {{crate_prefix_snake}}_config::ENV_PREFIX;
use {{crate_prefix_snake}}_config::EnvSource;
use {{crate_prefix_snake}}_config::MapEnv;
use {{crate_prefix_snake}}_runtime::{ShutdownReport, StopSignal};

struct Finished {
    report: ShutdownReport,
    anchor: PathBuf,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_assemblies_run_concurrently_without_interference() {
    let temp_a = TempDir::new().expect("临时目录 A");
    let temp_b = TempDir::new().expect("临时目录 B");

    let stop_a = StopSignal::new();
    let stop_b = StopSignal::new();
    // 日志级别用环境变量覆盖区分两套装配——顺带证明覆盖是按"各自的 MapEnv"走的。
    let (assembly_a, _view_a) = build(temp_a.path(), "info");
    let (assembly_b, mut view_b) = build(temp_b.path(), "warn");

    let anchor_a = temp_a.path().to_path_buf();
    let anchor_b = temp_b.path().to_path_buf();
    let (run_stop_a, run_stop_b) = (stop_a.clone(), stop_b.clone());
    let thread_a = std::thread::spawn(move || run_blocking(assembly_a, run_stop_a, anchor_a));
    let thread_b = std::thread::spawn(move || run_blocking(assembly_b, run_stop_b, anchor_b));

    // A 先停：B 必须活着，并且仍然响应自己的配置变更。
    stop_a.request_drain();
    let finished_a = thread_a.join().expect("线程 A");
    assert!(finished_a.report.clean(), "{:?}", finished_a.report);
    assert_eq!(exit_code(&finished_a.report), ProcessExit::Clean);

    // 改 B 的业务配置（半热段，不受 B 的环境变量覆盖影响）：
    // 注意 A 的锚点与 B 无关，这里只动 B 自己的文件。
    let _ = &finished_a.anchor;
    let config_b = temp_b.path().join("config.toml");
    std::fs::write(
        &config_b,
        "[supervisor]\nrestart_backoff_ms = 900\nrestart_backoff_cap_ms = 30000\n",
    )
    .expect("改 B 的配置");
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if view_b.borrow().supervisor.restart_backoff_ms == 900 {
                break;
            }
            view_b.changed().await.expect("B 的视图通道");
        }
    })
    .await;
    assert!(changed.is_ok(), "A 停止之后 B 的配置重载必须照常工作");
    assert_eq!(
        view_b.borrow().log.filter,
        "warn",
        "B 的环境变量覆盖只影响 B（A 用 info，B 用 warn），且优先级高于文件"
    );

    stop_b.request_drain();
    let finished_b = thread_b.join().expect("线程 B");
    assert!(finished_b.report.clean(), "{:?}", finished_b.report);

    // 数据库互不干扰：各自锚点下有自己的库文件。
    assert!(
        temp_a.path().join("data/service.db").exists(),
        "A 的库应在 A 的锚点下"
    );
    assert!(
        temp_b.path().join("data/service.db").exists(),
        "B 的库应在 B 的锚点下"
    );
}

fn build(anchor_dir: &Path, filter: &str) -> (Assembly, watch::Receiver<Config>) {
    let env: Arc<dyn EnvSource> =
        Arc::new(MapEnv::new().with(format!("{ENV_PREFIX}_LOG__FILTER"), filter));
    let telemetry = Arc::new(Telemetry::new(filter).expect("日志过滤器"));
    let settings = resolve_with_anchor(&Args { config: None }, &env, Anchor::from_dir(anchor_dir))
        .expect("设置解析");
    let assembly = Assembly::prepare(settings, env, telemetry).expect("装配");
    let view = assembly.config_view();
    (assembly, view)
}

fn run_blocking(assembly: Assembly, stop: StopSignal, anchor: PathBuf) -> Finished {
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let report = runtime
        .block_on(assembly.run(&runtime, stop))
        .expect("装配应当跑到关停")
        .report;
    runtime.shutdown_timeout(Duration::from_secs(1));
    Finished { report, anchor }
}
