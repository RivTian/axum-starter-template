//! 二进制入口的编排：telemetry 安装、runtime、看门狗、退出码。
//!
//! 这里**不做业务**，也不碰库 crate 的内部：只把设置、装配与进程级资源（runtime、全局 subscriber、
//! 看门狗）按顺序摆好。库路径（集成测试）走 `Assembly`，不会经过这里——所以测试不需要装全局 subscriber。

use std::sync::Arc;
use std::time::Duration;

use {{crate_prefix_snake}}_config::ENV_PREFIX;
use {{crate_prefix_snake}}_runtime::{ShutdownReport, StopSignal, StopTrigger};

use crate::assembly::Assembly;
use crate::settings::{Args, process_env, resolve};
use crate::telemetry::Telemetry;

/// L0 看门狗预算：超过它就不再等关停（打印后强制退出，退出码 4）。
pub const WATCHDOG_BUDGET: Duration = Duration::from_secs(15);
/// L2f：主 runtime 的 `shutdown_timeout`（同步段，不可加速）。
pub const MAIN_RUNTIME_SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);

/// 进程退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessExit {
    /// 0：干净关停。
    Clean,
    /// 2：启动失败（配置/资源/绑定/注册）。
    Startup,
    /// 3：fatal 任务自行退出或重启预算耗尽。
    TaskFailure,
    /// 4：关停未完成（强推、仍有任务收不回来、资源关闭失败）。
    IncompleteShutdown,
}

impl ProcessExit {
    pub const fn code(self) -> u8 {
        match self {
            Self::Clean => 0,
            Self::Startup => 2,
            Self::TaskFailure => 3,
            Self::IncompleteShutdown => 4,
        }
    }
}

impl From<ProcessExit> for std::process::ExitCode {
    fn from(exit: ProcessExit) -> Self {
        Self::from(exit.code())
    }
}

/// 关停报告 → 退出码。
pub fn exit_code(report: &ShutdownReport) -> ProcessExit {
    if !report.clean() {
        return ProcessExit::IncompleteShutdown;
    }
    if matches!(report.trigger, StopTrigger::TaskFailure { .. }) {
        return ProcessExit::TaskFailure;
    }
    ProcessExit::Clean
}

/// 看门狗：到点调用 `on_timeout`；被 abort 则什么都不做。
///
/// 抽成函数是为了能用虚拟时钟测（二进制路径传 `std::process::exit`，测试传一个标记）。
pub async fn watchdog<F: FnOnce()>(budget: Duration, on_timeout: F) {
    tokio::time::sleep(budget).await;
    on_timeout();
}

/// 构建串：启动首条日志的内容。
pub fn build_string() -> String {
    format!(
        "{} {} git={} dirty={} profile={}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_GIT_SHA"),
        env!("BUILD_GIT_DIRTY"),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    )
}

/// 二进制入口。`main` 只调用它并映射退出码。
pub fn run(args: Args) -> ProcessExit {
    let env = process_env();
    let settings = match resolve(&args, &env) {
        Ok(settings) => settings,
        Err(err) => {
            // 日志还没起来：这是唯一允许直接写 stderr 的地方。
            eprintln!("startup failed: {err}");
            return ProcessExit::Startup;
        }
    };

    let mut telemetry = match Telemetry::new(&settings.config.log.filter) {
        Ok(telemetry) => telemetry,
        Err(err) => {
            eprintln!("startup failed: {err}");
            return ProcessExit::Startup;
        }
    };
    if let Err(err) = telemetry.install() {
        eprintln!("startup failed: {err}");
        return ProcessExit::Startup;
    }
    let telemetry = Arc::new(telemetry);

    // 首条事件固定是构建串：先回答"跑的是哪份代码"。
    tracing::info!(build = %build_string(), "starting");
    tracing::info!(env_prefix = ENV_PREFIX, "configuration environment prefix");
    telemetry.log_notices(&settings.notices);
    if settings.from_embedded_template {
        tracing::warn!(
            path = %settings.source.path.display(),
            "configuration file was missing: wrote the embedded default template"
        );
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!(error = %err, "failed to build the main runtime");
            return ProcessExit::Startup;
        }
    };

    let assembly = match Assembly::prepare(settings, env, telemetry.clone()) {
        Ok(assembly) => assembly,
        Err(err) => {
            tracing::error!(error = %err, "failed to prepare the assembly");
            return ProcessExit::Startup;
        }
    };

    let stop = StopSignal::new();
    let watchdog_task = runtime.spawn(watchdog(WATCHDOG_BUDGET, || {
        eprintln!(
            "shutdown did not finish within the watchdog budget ({}s): forcing exit",
            WATCHDOG_BUDGET.as_secs()
        );
        std::process::exit(i32::from(ProcessExit::IncompleteShutdown.code()));
    }));

    let outcome = runtime.block_on(assembly.run(&runtime, stop));
    watchdog_task.abort();

    let exit = match outcome {
        Ok(outcome) => {
            telemetry.log_shutdown(&outcome.report);
            exit_code(&outcome.report)
        }
        Err(err) => {
            tracing::error!(error = %err, "startup failed after logging was up");
            ProcessExit::Startup
        }
    };

    // 附加 runtime 的逆序关闭点：骨架只有一个主 runtime；加 aux 时在这里对它
    // `shutdown_timeout`（逆序、同步），最后才是主 runtime——见 crates/runtime/README.md 的准入清单。
    runtime.shutdown_timeout(MAIN_RUNTIME_SHUTDOWN_BUDGET);
    exit
}
