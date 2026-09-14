//! Synchronous ownership of every runtime. Handles are resolved only during
//! startup; worker/API crates receive neither a runtime owner nor a handle map.

use crate::config::{Binding, BootConfig, ConfigError, RuntimeSpec};
use std::borrow::Cow;
use std::time::{Duration, Instant};
use tokio::runtime::{Builder, Handle, Runtime};

pub(crate) struct RuntimeSet {
    main: Option<Runtime>,
    extra: Vec<(String, Runtime)>,
    emergency_budget: Duration,
    shutdown_deadline: Option<Instant>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildError {
    #[error(transparent)]
    Invalid(#[from] ConfigError),
    #[error("runtime {name} construction failed (cleanup budget exhausted: {cleanup_exhausted})")]
    Create {
        name: String,
        #[source]
        source: std::io::Error,
        cleanup_exhausted: bool,
    },
}

pub(crate) struct Executors {
    main: Handle,
    extra: Vec<(String, Handle)>,
}
pub(crate) struct Target<'a> {
    pub(crate) handle: &'a Handle,
    pub(crate) name: Cow<'static, str>,
}

#[derive(Debug, thiserror::Error)]
#[error("requested runtime was not constructed; no fallback is allowed")]
pub(crate) struct ResolveError;

impl Executors {
    pub(crate) fn resolve(&self, binding: &Binding) -> Result<Target<'_>, ResolveError> {
        match binding {
            Binding::Main => Ok(Target {
                handle: &self.main,
                name: Cow::Borrowed("main"),
            }),
            Binding::Extra(name) => self
                .extra
                .iter()
                .find(|(key, _)| key == name)
                .map(|(name, handle)| Target {
                    handle,
                    name: Cow::Owned(name.clone()),
                })
                .ok_or(ResolveError),
        }
    }
    #[cfg(test)]
    pub(crate) fn current() -> Self {
        Self {
            main: Handle::current(),
            extra: Vec::new(),
        }
    }
}

fn create_runtime(name: &str, spec: &RuntimeSpec) -> std::io::Result<Runtime> {
    // Every executor drives itself. One worker means multi_thread(1), never a
    // current_thread runtime whose tasks would wait for another block_on driver.
    Builder::new_multi_thread()
        .enable_all()
        .worker_threads(spec.worker_threads)
        .max_blocking_threads(spec.max_blocking_threads)
        .thread_name(format!("service-{name}"))
        .build()
}

impl RuntimeSet {
    pub(crate) fn build(config: &BootConfig) -> Result<Self, BuildError> {
        Self::build_with(config, create_runtime)
    }

    fn build_with<F>(config: &BootConfig, mut create: F) -> Result<Self, BuildError>
    where
        F: FnMut(&str, &RuntimeSpec) -> std::io::Result<Runtime>,
    {
        // Also protects programmatic callers that bypass the file loader.
        config.validate_runtimes()?;
        let mut owned = Self {
            main: None,
            extra: Vec::new(),
            emergency_budget: config.shutdown.runtime,
            shutdown_deadline: None,
        };
        let specs = std::iter::once(("main", config.main_spec())).chain(
            config
                .extra_runtimes
                .iter()
                .map(|(name, spec)| (name.as_str(), spec.clone())),
        );
        for (name, spec) in specs {
            match create(name, &spec) {
                Ok(runtime) => {
                    if name == "main" {
                        owned.main = Some(runtime);
                    } else {
                        owned.extra.push((name.to_owned(), runtime));
                    }
                    tracing::info!(
                        event = "runtime_built",
                        runtime = name,
                        worker_threads = spec.worker_threads,
                        max_blocking_threads = spec.max_blocking_threads,
                        "runtime built"
                    );
                }
                Err(source) => {
                    let now = Instant::now();
                    let deadline = now.checked_add(config.shutdown.runtime).unwrap_or(now);
                    let cleanup_exhausted = owned.shutdown(deadline);
                    return Err(BuildError::Create {
                        name: name.to_owned(),
                        source,
                        cleanup_exhausted,
                    });
                }
            }
        }
        let (workers, blocking) = config.thread_totals().expect("validated totals");
        // These budgets are not process thread counts or CPU quotas.
        tracing::info!(
            event = "runtime_topology",
            runtime_count = owned.extra.len() + 1,
            worker_threads_total = workers,
            max_blocking_threads_total = blocking,
            ticker_runtime = config.ticker_runtime.as_str(),
            http_runtime = config.http_runtime.as_str(),
            "runtime topology ready"
        );
        Ok(owned)
    }

    pub(crate) fn executors(&self) -> Executors {
        Executors {
            main: self
                .main
                .as_ref()
                .expect("main runtime already consumed")
                .handle()
                .clone(),
            extra: self
                .extra
                .iter()
                .map(|(name, runtime)| (name.clone(), runtime.handle().clone()))
                .collect(),
        }
    }
    pub(crate) fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.main
            .as_ref()
            .expect("main runtime already consumed")
            .block_on(future)
    }
    pub(crate) fn shutdown(&mut self, deadline: Instant) -> bool {
        self.shutdown_with(deadline, |name, runtime, remaining| {
            runtime.shutdown_timeout(remaining);
            tracing::info!(
                event = "runtime_shutdown_returned",
                runtime = name,
                budget_exhausted = remaining.is_zero() || Instant::now() >= deadline,
                "runtime shutdown wait returned"
            );
        })
    }
    fn shutdown_with<F>(&mut self, deadline: Instant, mut close: F) -> bool
    where
        F: FnMut(&str, Runtime, Duration),
    {
        // Preserve the first cutoff even if a teardown observer/logger unwinds
        // and Drop has to finish closing the remaining runtimes.
        let deadline = self
            .shutdown_deadline
            .map_or(deadline, |first| first.min(deadline));
        self.shutdown_deadline = Some(deadline);
        let mut exhausted = false;
        // BTreeMap construction order, reversed; shared resources' main driver last.
        while let Some((name, runtime)) = self.extra.pop() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            exhausted |= remaining.is_zero();
            close(&name, runtime, remaining);
            exhausted |= Instant::now() >= deadline;
        }
        if let Some(runtime) = self.main.take() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            exhausted |= remaining.is_zero();
            close("main", runtime, remaining);
            exhausted |= Instant::now() >= deadline;
        }
        exhausted
    }
}
impl Drop for RuntimeSet {
    fn drop(&mut self) {
        // Owners are restricted to synchronous main/tests. Returned errors and
        // coordinator unwind use explicit shutdown with the preserved timeline.
        if self.main.is_some() || !self.extra.is_empty() {
            let now = Instant::now();
            let deadline = self
                .shutdown_deadline
                .unwrap_or_else(|| now.checked_add(self.emergency_budget).unwrap_or(now));
            self.shutdown(deadline);
        }
    }
}

#[cfg(test)]
mod tests;
