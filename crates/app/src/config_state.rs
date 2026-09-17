//! 生效配置的唯一发布口：把**三个档位的副作用**与 watch 视图绑在一起。
//!
//! 三条纪律：
//! - 只有一个写入口 [`ConfigState::apply`]：先执行副作用（可热段 / 半热段），再发布新配置；
//! - 发布的就是生效值——cold 段在 config crate 里已经被回滚成运行值，这里不要再"挑字段";
//! - 可热段的应用要**可能失败**（比如日志过滤器非法）：失败就整次重载作废，保留 last-good。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use {{crate_prefix_snake}}_config::{Config, ReloadOutcome, ReloadReport};
use {{crate_prefix_snake}}_core::Error;
use {{crate_prefix_snake}}_runtime::Backoff;
use {{crate_prefix_snake}}_runtime::SharedBackoff;
use {{crate_prefix_snake}}_runtime::set_shared_backoff;

use crate::telemetry::Telemetry;

/// 配置的运行态视图。
pub struct ConfigState {
    sender: watch::Sender<Config>,
    telemetry: Arc<Telemetry>,
    backoff: SharedBackoff,
}

impl ConfigState {
    /// 启动时构造：把初始配置发布进 watch（第一份生效值）。
    pub fn new(config: Config, telemetry: Arc<Telemetry>, backoff: SharedBackoff) -> Self {
        let (sender, _) = watch::channel(config);
        Self {
            sender,
            telemetry,
            backoff,
        }
    }

    /// 订阅生效配置（任务面读配置都走这里）。
    pub fn subscribe(&self) -> watch::Receiver<Config> {
        self.sender.subscribe()
    }

    /// 当前生效配置。
    pub fn current(&self) -> Config {
        self.sender.borrow().clone()
    }

    /// 唯一写入口：应用一次重载结果。
    ///
    /// 顺序：可热副作用（可能失败）→ 半热副作用 → 发布。任何一步失败都不发布，last-good 不变。
    pub fn apply(&self, outcome: ReloadOutcome) -> Result<ReloadReport, Error> {
        let ReloadOutcome {
            config,
            report,
            notices,
        } = outcome;
        self.apply_hot(&config)?;
        self.apply_semi(&config);
        self.sender.send_replace(config);
        self.telemetry.log_notices(&notices);
        Ok(report)
    }

    /// 可热：日志过滤器（reload 句柄；非法的过滤器会在这里被拒绝）。
    fn apply_hot(&self, config: &Config) -> Result<(), Error> {
        self.telemetry.apply_filter(&config.log.filter)
    }

    /// 半热：重启退避参数（下一次调度重启时生效）。
    fn apply_semi(&self, config: &Config) {
        set_shared_backoff(
            &self.backoff,
            Backoff {
                base: Duration::from_millis(config.supervisor.restart_backoff_ms),
                cap: Duration::from_millis(config.supervisor.restart_backoff_cap_ms),
            },
        );
    }
}

impl std::fmt::Debug for ConfigState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigState").finish_non_exhaustive()
    }
}
