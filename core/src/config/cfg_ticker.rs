//! 示例任务面 `ticker` 的子配置（`[ticker]` 段）
//!
//! 它是模板里「一个面的配置段」的参照：一个启动闸（半热）+ 一个可热的节奏参数。
//! 把它改成自己的面时，段名、字段、钳位区间一起改；面的 `runtime` 绑定字段随
//! 多 runtime 片进场。
//!
//! # 热度
//!
//! - `enabled`：半热（启动闸）。启动时关掉就不注册任务，热重载改成 true **不会**
//!   补拉起——`ReloadReport` 把它归入 `deferred`，日志明说要重启；
//! - `interval_ms`：可热。任务每 tick 经 `ConfigHandle::current()` 现读，下一 tick 生效；
//! - `runtime`：不可热。绑定在 spawn 那一刻定死。

use serde::{Deserialize, Serialize};

use super::clamp::clamp_field;

/// `[ticker]` 段完整配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TickerConfig {
    /// 启动闸：false 时不注册 ticker 任务（启动日志会 warn）
    pub enabled: bool,
    /// 打点周期（毫秒）；可热
    pub interval_ms: u64,
    /// 绑定到哪个附加 runtime（`[runtime.extra.<name>]` 的名字）；缺省主 runtime
    pub runtime: Option<String>,
}

impl Default for TickerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_ms: 1_000,
            runtime: None,
        }
    }
}

impl TickerConfig {
    /// 下限 100ms：再短就是忙等；上限一小时：再长就该改成定时任务而不是常驻面
    pub(super) fn sanitize(&mut self) {
        clamp_field(
            &mut self.interval_ms,
            100,
            3_600_000,
            "ticker",
            "interval_ms",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_yields_defaults() {
        let cfg: TickerConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, TickerConfig::default());
        assert!(cfg.enabled);
    }

    #[test]
    fn sanitize_clamps_interval() {
        let mut cfg = TickerConfig {
            interval_ms: 1,
            ..TickerConfig::default()
        };
        cfg.sanitize();
        assert_eq!(cfg.interval_ms, 100);
    }
}
