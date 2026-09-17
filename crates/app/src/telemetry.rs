//! 可观测性：进程内唯一的 subscriber、结构记录的日志落点与 panic hook。
//!
//! 两条纪律：
//! - 库 crate 只发事件、不装 subscriber；装 subscriber 的只有这里（装配层），而且一个进程只装一次。
//! - 日志形态只有一种（`tracing_subscriber::fmt` 的单行格式），不提供格式开关。
//!
//! `Telemetry::new` 只**构造**（校验过滤器、拿到 reload 句柄），不安装；安装是 [`Telemetry::install`]，
//! 只有二进制入口会调用它——测试因此可以构造 Telemetry 而不污染进程的全局 subscriber。

use std::sync::Once;

use tracing_subscriber::Registry;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::util::TryInitError;

use {{crate_prefix_snake}}_config::Notice;
use {{crate_prefix_snake}}_core::{Error, ErrorKind};
use {{crate_prefix_snake}}_runtime::ExitRecord;
use {{crate_prefix_snake}}_runtime::ShutdownReport;
use {{crate_prefix_snake}}_runtime::StopTrigger;

static PANIC_HOOK: Once = Once::new();

/// 进程级日志门面。
pub struct Telemetry {
    handle: reload::Handle<EnvFilter, Registry>,
    subscriber: Option<Box<dyn tracing::Subscriber + Send + Sync>>,
}

impl Telemetry {
    /// 构造：校验 `filter`（EnvFilter 语法），并准备可热重载的 subscriber。
    pub fn new(filter: &str) -> Result<Self, Error> {
        let env_filter = parse_filter(filter)?;
        let (layer, handle) = reload::Layer::new(env_filter);
        let subscriber = Registry::default()
            .with(layer)
            // 一种形态：单行、带 target、不带 ANSI 颜色（日志文件/采集器不需要转义序列）。
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_ansi(false),
            );
        let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = Box::new(subscriber);
        Ok(Self {
            handle,
            subscriber: Some(subscriber),
        })
    }

    /// 安装为进程级 subscriber（幂等：第二次调用返回错误，不 panic）。顺带装 panic hook。
    pub fn install(&mut self) -> Result<(), Error> {
        let Some(subscriber) = self.subscriber.take() else {
            return Err(Error::new(
                ErrorKind::Startup,
                "日志 subscriber 已经安装过了",
            ));
        };
        subscriber.try_init().map_err(|err: TryInitError| {
            Error::with_source(ErrorKind::Startup, "安装全局日志 subscriber 失败", err)
        })?;
        install_panic_hook();
        Ok(())
    }

    /// 应用日志过滤器：启动与热重载走**同一个**函数（可热段就落在这里）。
    pub fn apply_filter(&self, filter: &str) -> Result<(), Error> {
        let env_filter = parse_filter(filter)?;
        self.handle
            .modify(|current| *current = env_filter)
            .map_err(|err| {
                Error::with_source(
                    ErrorKind::Task,
                    "热改日志过滤器失败（reload 句柄已失效）",
                    err,
                )
            })
    }

    /// 把一条退出记录打成日志：这是唯一知道日志字段形状的地方。
    pub fn log_exit(&self, record: &ExitRecord) {
        tracing::info!(
            task = %record.key,
            runtime = %record.runtime,
            incarnation = record.incarnation,
            cause = record.cause.as_str(),
            outcome = record.outcome.as_str(),
            uptime_ms = record.uptime.as_millis() as u64,
            restarts = record.restarts,
            "task exited"
        );
    }

    /// 关停报告：`stopped` 一行一个（任务名 + runtime 都在，runtime 从退出记录里取），
    /// 收不回来的用 warn。
    pub fn log_shutdown(&self, report: &ShutdownReport) {
        for key in &report.stopped {
            let runtime = report
                .records
                .iter()
                .rev()
                .find(|record| &record.key == key)
                .map_or_else(|| "unknown".to_owned(), |record| record.runtime.to_string());
            tracing::info!(task = %key, runtime = %runtime, "stopped");
        }
        for key in &report.still_running {
            tracing::warn!(task = %key, action = "aborted", "task did not stop within budget");
        }
        if let StopTrigger::TaskFailure { key } = &report.trigger {
            tracing::error!(task = %key, "fatal task exit triggered shutdown");
        }
        for resource in &report.resources_closed {
            tracing::info!(resource = %resource, "resource closed");
        }
        for failure in &report.resource_failures {
            tracing::error!(resource = %failure, "resource close failed");
        }
        tracing::info!(
            forced = report.forced,
            elapsed_ms = report.elapsed.as_millis() as u64,
            "shutdown finished"
        );
    }

    /// 管线提示（钳位、遮蔽、默认模板写不进去）统一在这里落日志。
    pub fn log_notices(&self, notices: &[Notice]) {
        for notice in notices {
            tracing::warn!(
                path = %notice.path,
                kind = notice.kind.as_str(),
                detail = %notice.detail,
                "configuration notice"
            );
        }
    }
}

fn parse_filter(filter: &str) -> Result<EnvFilter, Error> {
    EnvFilter::try_new(filter).map_err(|err| {
        Error::with_source(
            ErrorKind::Config,
            format!("log.filter 不是合法的过滤器：`{filter}`"),
            err,
        )
    })
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            tracing::error!(panic = %info, "panic");
        }));
    });
}
