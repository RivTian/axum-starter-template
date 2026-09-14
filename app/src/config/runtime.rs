//! Cold runtime topology. Validate the complete graph before starting any threads.

use super::{BootConfig, ConfigError};
use serde::Deserialize;

pub(crate) const MAX_EXTRA_RUNTIMES: usize = 8;
pub(crate) const MAX_WORKERS_TOTAL: usize = 256;
pub(crate) const MAX_BLOCKING_TOTAL: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeSpec {
    pub(crate) worker_threads: usize,
    pub(crate) max_blocking_threads: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Binding {
    Main,
    Extra(String),
}
impl Binding {
    pub(crate) fn parse(value: Option<String>, field: &'static str) -> Result<Self, ConfigError> {
        match value {
            None => Ok(Self::Main),
            Some(name) if name == "main" => Ok(Self::Main),
            Some(name) if valid_name(&name) => Ok(Self::Extra(name)),
            Some(_) => Err(ConfigError::new(
                field,
                "runtime name must be lowercase kebab-case, at most 32 bytes",
            )),
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Main => "main",
            Self::Extra(name) => name,
        }
    }
}

fn valid_name(name: &str) -> bool {
    name.len() <= 32
        && name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

impl BootConfig {
    pub(crate) fn main_spec(&self) -> RuntimeSpec {
        RuntimeSpec {
            worker_threads: self.worker_threads,
            max_blocking_threads: self.max_blocking_threads,
        }
    }
    pub(crate) fn thread_totals(&self) -> Option<(usize, usize)> {
        self.extra_runtimes.values().try_fold(
            (self.worker_threads, self.max_blocking_threads),
            |(workers, blocking), spec| {
                Some((
                    workers.checked_add(spec.worker_threads)?,
                    blocking.checked_add(spec.max_blocking_threads)?,
                ))
            },
        )
    }
    pub(crate) fn validate_runtimes(&self) -> Result<(), ConfigError> {
        if self.extra_runtimes.len() > MAX_EXTRA_RUNTIMES {
            return Err(ConfigError::new(
                "runtime.extra",
                "at most 8 extra runtimes are allowed",
            ));
        }
        if !(1..=MAX_WORKERS_TOTAL).contains(&self.worker_threads)
            || !(1..=MAX_BLOCKING_TOTAL).contains(&self.max_blocking_threads)
        {
            return Err(ConfigError::new(
                "runtime",
                "main thread budgets must be in [1, 256]",
            ));
        }
        for (name, spec) in &self.extra_runtimes {
            if name == "main" || !valid_name(name) {
                return Err(ConfigError::new(
                    "runtime.extra",
                    "extra names must be lowercase kebab-case up to 32 bytes; main is reserved",
                ));
            }
            if !(1..=MAX_WORKERS_TOTAL).contains(&spec.worker_threads)
                || !(1..=MAX_BLOCKING_TOTAL).contains(&spec.max_blocking_threads)
            {
                return Err(ConfigError::new(
                    format!("runtime.extra.{name}"),
                    "thread budgets must be in [1, 256]",
                ));
            }
            if self.http_runtime.as_str() != name && self.ticker_runtime.as_str() != name {
                return Err(ConfigError::new(
                    format!("runtime.extra.{name}"),
                    "runtime has no bound task",
                ));
            }
        }
        for (field, binding) in [
            ("http.runtime", &self.http_runtime),
            ("ticker.runtime", &self.ticker_runtime),
        ] {
            if let Binding::Extra(name) = binding
                && (!valid_name(name) || !self.extra_runtimes.contains_key(name))
            {
                return Err(ConfigError::new(
                    field,
                    "binding references an undeclared extra runtime",
                ));
            }
        }
        let totals = self
            .thread_totals()
            .ok_or_else(|| ConfigError::new("runtime", "thread budget sum overflow"))?;
        if totals.0 > MAX_WORKERS_TOTAL || totals.1 > MAX_BLOCKING_TOTAL {
            return Err(ConfigError::new(
                "runtime",
                "total worker and blocking budgets must each be at most 256",
            ));
        }
        Ok(())
    }
}
