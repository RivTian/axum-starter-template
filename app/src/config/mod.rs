//! Application configuration assembly, not a schema forced into every component.
//! Startup and reload share this pipeline; cold assembly values never enter watch.

mod load;
mod runtime;
pub(crate) use runtime::{Binding, RuntimeSpec};
pub(crate) mod reload;
pub(crate) use load::{DEFAULT_PATH, LoadContext, load, select_path};

use crate::shutdown::Budgets;
use serde::Deserialize;
use service_api::HttpSettings;
use service_core::TickerInterval;
use service_core::config::HotConfig;
use service_storage::StorageConfig;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) boot: BootConfig,
    pub(crate) hot: HotConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BootConfig {
    pub(crate) http: HttpSettings,
    pub(crate) storage: StorageConfig,
    pub(crate) worker_threads: usize,
    pub(crate) max_blocking_threads: usize,
    pub(crate) extra_runtimes: BTreeMap<String, RuntimeSpec>,
    pub(crate) http_runtime: Binding,
    pub(crate) ticker_runtime: Binding,
    pub(crate) startup_timeout: Duration,
    pub(crate) reload_timeout: Duration,
    pub(crate) shutdown: Budgets,
}

impl BootConfig {
    pub(crate) fn changed_fields(&self, new: &Self) -> Vec<&'static str> {
        // Exhaustive destructuring makes a new cold field require an explicit
        // reporting decision. The equality gate still rejects all cold changes.
        let Self {
            http,
            storage,
            worker_threads,
            max_blocking_threads,
            extra_runtimes,
            http_runtime,
            ticker_runtime,
            startup_timeout,
            reload_timeout,
            shutdown,
        } = self;
        let mut fields = Vec::new();
        if http != &new.http {
            fields.push("http");
        }
        if storage != &new.storage {
            fields.push("storage");
        }
        if worker_threads != &new.worker_threads {
            fields.push("runtime.worker_threads");
        }
        if max_blocking_threads != &new.max_blocking_threads {
            fields.push("runtime.max_blocking_threads");
        }
        if extra_runtimes != &new.extra_runtimes {
            fields.push("runtime.extra");
        }
        if http_runtime != &new.http_runtime {
            fields.push("http.runtime");
        }
        if ticker_runtime != &new.ticker_runtime {
            fields.push("ticker.runtime");
        }
        if startup_timeout != &new.startup_timeout {
            fields.push("lifecycle.startup_timeout_ms");
        }
        if reload_timeout != &new.reload_timeout {
            fields.push("lifecycle.reload_timeout_ms");
        }
        let Budgets {
            grace,
            abort_reap,
            storage_close,
            runtime,
        } = shutdown;
        if grace != &new.shutdown.grace {
            fields.push("lifecycle.grace_ms");
        }
        if abort_reap != &new.shutdown.abort_reap {
            fields.push("lifecycle.abort_reap_ms");
        }
        if storage_close != &new.shutdown.storage_close {
            fields.push("lifecycle.storage_close_ms");
        }
        if runtime != &new.shutdown.runtime {
            fields.push("lifecycle.runtime_shutdown_ms");
        }
        fields
    }
}

#[derive(Debug, thiserror::Error)]
#[error("configuration {field}: {message}")]
pub(crate) struct ConfigError {
    field: String,
    message: &'static str,
}
impl ConfigError {
    pub(crate) fn new(field: impl Into<String>, message: &'static str) -> Self {
        // Diagnostics identify fields, never raw values, file contents or env values.
        let field = field
            .into()
            .chars()
            .take(200)
            .map(|c| if c.is_control() { '?' } else { c })
            .collect();
        Self { field, message }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    http: RawHttp,
    storage: RawStorage,
    ticker: RawTicker,
    runtime: RawRuntime,
    lifecycle: RawLifecycle,
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawHttp {
    runtime: Option<String>,
    listen: String,
    request_timeout_ms: u64,
    ready_probe_timeout_ms: u64,
}
impl Default for RawHttp {
    fn default() -> Self {
        Self {
            runtime: None,
            listen: "127.0.0.1:0".into(),
            request_timeout_ms: 2000,
            ready_probe_timeout_ms: 500,
        }
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawStorage {
    path: String,
    min_connections: u32,
    max_connections: u32,
    acquire_timeout_ms: u64,
    busy_timeout_ms: u64,
}
impl Default for RawStorage {
    fn default() -> Self {
        Self {
            path: "../data/service.sqlite3".into(),
            min_connections: 0,
            max_connections: 4,
            acquire_timeout_ms: 2000,
            busy_timeout_ms: 1000,
        }
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTicker {
    runtime: Option<String>,
    interval_ms: u64,
}
impl Default for RawTicker {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            runtime: None,
        }
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRuntime {
    extra: BTreeMap<String, RuntimeSpec>,
    worker_threads: Option<usize>,
    max_blocking_threads: usize,
}
impl Default for RawRuntime {
    fn default() -> Self {
        Self {
            extra: BTreeMap::new(),
            worker_threads: None,
            max_blocking_threads: 16,
        }
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLifecycle {
    startup_timeout_ms: u64,
    reload_timeout_ms: u64,
    grace_ms: u64,
    abort_reap_ms: u64,
    storage_close_ms: u64,
    runtime_shutdown_ms: u64,
}
impl Default for RawLifecycle {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 10000,
            reload_timeout_ms: 2000,
            grace_ms: 5000,
            abort_reap_ms: 1000,
            storage_close_ms: 3000,
            runtime_shutdown_ms: 1000,
        }
    }
}

fn duration(field: &'static str, millis: u64) -> Result<Duration, ConfigError> {
    if !(1..=3_600_000).contains(&millis) {
        return Err(ConfigError::new(field, "duration must be in [1ms, 1h]"));
    }
    Ok(Duration::from_millis(millis))
}

// Complete/normalize before validate. Component-specific invariants stay with
// component constructors; only cross-component rules are owned here.
fn resolve(raw: RawConfig, context: &LoadContext) -> Result<Config, ConfigError> {
    let request = duration("http.request_timeout_ms", raw.http.request_timeout_ms)?;
    let ready = duration(
        "http.ready_probe_timeout_ms",
        raw.http.ready_probe_timeout_ms,
    )?;
    let listen: SocketAddr = raw.http.listen.parse().map_err(|_| {
        ConfigError::new(
            "http.listen",
            "expected an IP socket address, including a port",
        )
    })?;
    let http = HttpSettings::new(listen, request, ready)
        .map_err(|_| ConfigError::new("http", "require 0 < ready probe < request <= 60s"))?;
    if raw.storage.path.is_empty() || raw.storage.path == ":memory:" {
        return Err(ConfigError::new(
            "storage.path",
            "a nonempty file path is required",
        ));
    }
    let path = std::path::PathBuf::from(raw.storage.path);
    let path = if path.is_absolute() {
        path
    } else {
        context.directory.join(path)
    };
    let storage = StorageConfig::new(
        path,
        raw.storage.max_connections,
        raw.storage.min_connections,
        duration("storage.acquire_timeout_ms", raw.storage.acquire_timeout_ms)?,
        duration("storage.busy_timeout_ms", raw.storage.busy_timeout_ms)?,
    )
    .map_err(|_| {
        ConfigError::new(
            "storage",
            "require a file, 0 <= min <= max <= 64, max > 0 and timeouts in (0, 60s]",
        )
    })?;
    if !(10..=3_600_000).contains(&raw.ticker.interval_ms) {
        return Err(ConfigError::new(
            "ticker.interval_ms",
            "period must be in [10ms, 1h]",
        ));
    }
    let ticker = TickerInterval::try_from(Duration::from_millis(raw.ticker.interval_ms))
        .map_err(|_| ConfigError::new("ticker.interval_ms", "invalid period"))?;
    let worker_threads = raw
        .runtime
        .worker_threads
        .unwrap_or(context.default_workers);
    if !(1..=256).contains(&worker_threads)
        || !(1..=256).contains(&raw.runtime.max_blocking_threads)
    {
        return Err(ConfigError::new(
            "runtime",
            "thread budgets must be in [1, 256]",
        ));
    }
    let startup_timeout = duration(
        "lifecycle.startup_timeout_ms",
        raw.lifecycle.startup_timeout_ms,
    )?;
    let shutdown = Budgets {
        grace: duration("lifecycle.grace_ms", raw.lifecycle.grace_ms)?,
        abort_reap: duration("lifecycle.abort_reap_ms", raw.lifecycle.abort_reap_ms)?,
        storage_close: duration("lifecycle.storage_close_ms", raw.lifecycle.storage_close_ms)?,
        runtime: duration(
            "lifecycle.runtime_shutdown_ms",
            raw.lifecycle.runtime_shutdown_ms,
        )?,
    };
    if http.request_timeout() >= shutdown.grace {
        return Err(ConfigError::new(
            "lifecycle.grace_ms",
            "must exceed the HTTP request timeout",
        ));
    }
    if shutdown.plan(Instant::now()).is_none() {
        return Err(ConfigError::new("lifecycle", "shutdown deadlines overflow"));
    }
    let reload_timeout = duration(
        "lifecycle.reload_timeout_ms",
        raw.lifecycle.reload_timeout_ms,
    )?;
    if reload_timeout > Duration::from_secs(60) {
        return Err(ConfigError::new(
            "lifecycle.reload_timeout_ms",
            "reload deadline must be in (0, 60s]",
        ));
    }
    let config = Config {
        boot: BootConfig {
            http,
            storage,
            worker_threads,
            max_blocking_threads: raw.runtime.max_blocking_threads,
            extra_runtimes: raw.runtime.extra,
            http_runtime: Binding::parse(raw.http.runtime, "http.runtime")?,
            ticker_runtime: Binding::parse(raw.ticker.runtime, "ticker.runtime")?,
            startup_timeout,
            reload_timeout,
            shutdown,
        },
        hot: HotConfig { ticker },
    };
    config.boot.validate_runtimes()?;
    Ok(config)
}

#[cfg(test)]
mod tests;
