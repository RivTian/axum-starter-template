//! One finite, owned blocking load at a time. The worker returns only a candidate;
//! classification/publication stays on the app coordinator, never in that worker.

use super::{BootConfig, Config, ConfigError};
use service_core::config::{ConfigHandle, ConfigPublisher, Publication};
use std::sync::Arc;
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;

pub(crate) const JOB_NAME: &str = "config_loader";
type LoadResult = Result<Config, ConfigError>;
type Loader = Arc<dyn Fn() -> LoadResult + Send + Sync>;
pub(crate) type Completion = Option<Result<LoadResult, JoinError>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Published,
    NoChange,
    RequiresRestart,
    Invalid,
    Timeout,
    DiscardedExpired,
    DiscardedShutdown,
    GenerationExhausted,
}
impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::NoChange => "no_change",
            Self::RequiresRestart => "requires_restart",
            Self::Invalid => "invalid",
            Self::Timeout => "timeout",
            Self::DiscardedExpired => "discarded_expired",
            Self::DiscardedShutdown => "discarded_shutdown",
            Self::GenerationExhausted => "generation_exhausted",
        }
    }
}
#[derive(Debug)]
pub(crate) struct ReloadReport {
    pub(crate) outcome: Outcome,
    pub(crate) generation: u64,
    pub(crate) fields: Vec<&'static str>,
    error: Option<ConfigError>,
}
impl ReloadReport {
    pub(crate) fn log(&self) {
        // No raw config or env values. ConfigError's Display is already redacted.
        if let Some(error) = &self.error {
            tracing::warn!(event = "config_reload", result = self.outcome.label(), generation = self.generation,
                fields = ?self.fields, error = %error, "configuration candidate rejected");
        } else {
            tracing::info!(event = "config_reload", result = self.outcome.label(), generation = self.generation,
                fields = ?self.fields, "configuration reload result");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReloadFailure {
    Panic,
    UnexpectedCancellation,
    Invariant,
}
impl ReloadFailure {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Panic => "loader_panic",
            Self::UnexpectedCancellation => "unexpected_loader_cancellation",
            Self::Invariant => "loader_ownership_invariant",
        }
    }
}

struct Flight {
    deadline: Instant,
    expired: bool,
}

pub(crate) struct ReloadController {
    boot: BootConfig,
    publisher: ConfigPublisher,
    loader: Loader,
    jobs: JoinSet<LoadResult>,
    flight: Option<Flight>,
    pending: bool,
    closing: bool,
    abort_requested: bool,
}
impl ReloadController {
    pub(crate) fn new<L>(initial: &Config, loader: L) -> Self
    where
        L: Fn() -> LoadResult + Send + Sync + 'static,
    {
        Self {
            boot: initial.boot.clone(),
            publisher: ConfigPublisher::new(initial.hot.clone()),
            loader: Arc::new(loader),
            jobs: JoinSet::new(),
            flight: None,
            pending: false,
            closing: false,
            abort_requested: false,
        }
    }
    pub(crate) fn reader(&self) -> ConfigHandle {
        self.publisher.subscribe()
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.flight.is_some() || !self.jobs.is_empty()
    }
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.flight
            .as_ref()
            .filter(|flight| !flight.expired && !self.closing)
            .map(|flight| flight.deadline)
    }

    pub(crate) fn request(&mut self) {
        if self.closing {
            return;
        }
        if self.is_busy() {
            if !self.pending {
                self.pending = true;
                tracing::info!(
                    event = "reload_coalesced",
                    "one follow-up reload is pending"
                );
            }
        } else {
            self.start();
        }
    }
    fn start(&mut self) {
        let loader = self.loader.clone();
        let deadline = Instant::now() + self.boot.reload_timeout;
        self.jobs.spawn_blocking(move || loader());
        self.flight = Some(Flight {
            deadline,
            expired: false,
        });
        self.pending = false;
        tracing::info!(
            event = "reload_started",
            generation = self.publisher.current().generation,
            "loading configuration candidate"
        );
    }

    /// Called before every monitor select, so signal floods cannot suppress the
    /// deadline. It invalidates the result, NOT the still-owned blocking job.
    pub(crate) fn expire_if_due(&mut self) -> Option<ReloadReport> {
        if self
            .deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.flight.as_mut().expect("active flight").expired = true;
            return Some(self.report(Outcome::Timeout));
        }
        None
    }
    pub(crate) async fn next_completion(&mut self) -> Completion {
        self.jobs.join_next().await
    }

    pub(crate) fn complete(
        &mut self,
        completion: Completion,
    ) -> Result<ReloadReport, ReloadFailure> {
        let flight = self.flight.take().ok_or(ReloadFailure::Invariant)?;
        // A panic is fatal even after timeout: do not hide a loader bug as expiry.
        let loaded = match completion {
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                self.stop();
                return Err(if error.is_panic() {
                    ReloadFailure::Panic
                } else {
                    ReloadFailure::UnexpectedCancellation
                });
            }
            None => {
                self.stop();
                return Err(ReloadFailure::Invariant);
            }
        };
        let report = if self.closing {
            self.report(Outcome::DiscardedShutdown)
        } else if flight.expired {
            self.report(Outcome::DiscardedExpired)
        } else if Instant::now() >= flight.deadline {
            self.report(Outcome::Timeout)
        } else {
            match loaded {
                Ok(candidate) => self.commit(candidate),
                Err(error) => ReloadReport {
                    error: Some(error),
                    ..self.report(Outcome::Invalid)
                },
            }
        };
        // Only a real coalesced user request can cause another load, not failure
        // recovery or an automatic retry. The previous job has actually joined.
        if self.pending && !self.closing {
            self.start();
        }
        Ok(report)
    }

    fn commit(&mut self, candidate: Config) -> ReloadReport {
        if self.closing {
            return self.report(Outcome::DiscardedShutdown);
        }
        if candidate.boot != self.boot {
            return ReloadReport {
                fields: self.boot.changed_fields(&candidate.boot),
                ..self.report(Outcome::RequiresRestart)
            };
        }
        match self.publisher.publish(candidate.hot) {
            Ok(Publication::Published(_)) => ReloadReport {
                fields: vec!["ticker.interval_ms"],
                ..self.report(Outcome::Published)
            },
            Ok(Publication::Unchanged(_)) => self.report(Outcome::NoChange),
            Err(_) => self.report(Outcome::GenerationExhausted),
        }
    }
    fn report(&self, outcome: Outcome) -> ReloadReport {
        ReloadReport {
            outcome,
            generation: self.publisher.current().generation,
            fields: Vec::new(),
            error: None,
        }
    }

    pub(crate) fn stop(&mut self) {
        self.closing = true;
        self.pending = false;
    }
    pub(crate) fn abort_remaining(&mut self) {
        self.abort_requested = true;
        self.jobs.abort_all();
    }

    /// Cleanup never commits or starts a pending job. A started blocking load
    /// cannot be killed by abort; its flight remains occupied until this receipt.
    pub(crate) fn discard_for_shutdown(&mut self, completion: Completion) -> bool {
        let had_flight = self.flight.take().is_some();
        self.pending = false;
        let clean = match completion {
            Some(Ok(_)) => true,
            Some(Err(error)) if error.is_cancelled() && self.abort_requested => true,
            _ => false,
        };
        if !clean || !had_flight {
            tracing::error!(
                event = "reload_cleanup_failed",
                "configuration job failed during shutdown"
            );
        } else {
            self.report(Outcome::DiscardedShutdown).log();
        }
        clean && had_flight
    }
}
// JoinSet Drop requests abort, but is never reported as a successful join.

#[cfg(test)]
mod tests;
