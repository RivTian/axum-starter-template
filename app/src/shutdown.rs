//! One monotonic timeline spans asynchronous cleanup and synchronous runtime drop.

use crate::supervisor::{TaskError, TaskExit};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budgets {
    pub(crate) grace: Duration,
    pub(crate) abort_reap: Duration,
    pub(crate) storage_close: Duration,
    pub(crate) runtime: Duration,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Plan {
    pub(crate) started: Instant,
    pub(crate) grace: Instant,
    pub(crate) abort_reap: Instant,
    pub(crate) storage_close: Instant,
    pub(crate) final_deadline: Instant,
    pub(crate) invalid: bool,
}
impl Budgets {
    pub(crate) fn plan(self, now: Instant) -> Option<Plan> {
        let grace = now.checked_add(self.grace)?;
        let abort_reap = grace.checked_add(self.abort_reap)?;
        let storage_close = abort_reap.checked_add(self.storage_close)?;
        let final_deadline = storage_close.checked_add(self.runtime)?;
        Some(Plan {
            started: now,
            grace,
            abort_reap,
            storage_close,
            final_deadline,
            invalid: false,
        })
    }
}

#[derive(Debug)]
pub(crate) enum StopCause {
    Signal,
    Task(TaskExit),
    Startup {
        phase: &'static str,
        source: TaskError,
    },
    StartupTimeout(&'static str),
    ControlClosed,
    ReloadFailure(&'static str),
    EmptyTaskSet,
    CoordinatorPanic,
}
impl StopCause {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::Task(exit) => {
                if exit.kind.returned() {
                    "unexpected_return"
                } else {
                    "task_failure"
                }
            }
            Self::Startup { .. } => "startup_failure",
            Self::StartupTimeout(_) => "startup_timeout",
            Self::ControlClosed => "control_closed",
            Self::ReloadFailure(_) => "reload_failure",
            Self::EmptyTaskSet => "empty_task_set",
            Self::CoordinatorPanic => "coordinator_panic",
        }
    }
}

#[derive(Default)]
pub(crate) struct ShutdownContext {
    pub(crate) plan: Option<Plan>,
    pub(crate) cause: Option<StopCause>,
    pub(crate) emergency_unreaped: Vec<&'static str>,
}
impl ShutdownContext {
    pub(crate) fn begin(&mut self, cause: StopCause, budgets: Budgets) {
        if self.cause.is_none() {
            self.cause = Some(cause);
        }
        if self.plan.is_none() {
            let now = Instant::now();
            // Invalid arithmetic must fail closed, never introduce an unbounded wait.
            self.plan = Some(budgets.plan(now).unwrap_or(Plan {
                started: now,
                grace: now,
                abort_reap: now,
                storage_close: now,
                final_deadline: now,
                invalid: true,
            }));
        }
    }
    pub(crate) fn runtime_deadline(&self, budget: Duration) -> Instant {
        let now = Instant::now();
        let cap = now.checked_add(budget).unwrap_or(now);
        self.plan.map_or(cap, |plan| plan.final_deadline.min(cap))
    }
}

pub(crate) fn stage_deadline(boundary: Instant, budget: Duration) -> tokio::time::Instant {
    let now = Instant::now();
    boundary.min(now.checked_add(budget).unwrap_or(now)).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageClose {
    NotCreated,
    Closed,
    TimedOut,
    Interrupted,
    SkippedUnproven,
}
impl StorageClose {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotCreated => "not_created",
            Self::Closed => "closed",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
            Self::SkippedUnproven => "skipped_unproven",
        }
    }
    fn successful(self) -> bool {
        matches!(self, Self::NotCreated | Self::Closed)
    }
}

pub(crate) struct RunReport {
    pub(crate) cause: StopCause,
    pub(crate) tasks: Vec<TaskExit>,
    pub(crate) unreaped: Vec<&'static str>,
    pub(crate) forced: bool,
    pub(crate) cleanup_failed: bool,
    pub(crate) storage: StorageClose,
}
impl RunReport {
    pub(crate) fn failed(cause: StopCause) -> Self {
        Self {
            cause,
            tasks: Vec::new(),
            unreaped: Vec::new(),
            forced: true,
            cleanup_failed: true,
            storage: StorageClose::SkippedUnproven,
        }
    }
    pub(crate) fn succeeded(&self) -> bool {
        matches!(self.cause, StopCause::Signal)
            && !self.forced
            && !self.cleanup_failed
            && self.unreaped.is_empty()
            && self.storage.successful()
            && self.tasks.iter().all(|task| task.kind.returned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_reason_and_deadlines_are_not_refreshed_by_later_failures() {
        let budget = Budgets {
            grace: Duration::from_secs(1),
            abort_reap: Duration::from_secs(1),
            storage_close: Duration::from_secs(1),
            runtime: Duration::from_secs(1),
        };
        let mut context = ShutdownContext::default();
        context.begin(StopCause::Signal, budget);
        let first = context.plan.unwrap();
        context.begin(StopCause::CoordinatorPanic, budget);
        assert!(matches!(context.cause, Some(StopCause::Signal)));
        assert_eq!(context.plan.unwrap().final_deadline, first.final_deadline);
        assert!(context.runtime_deadline(Duration::from_millis(10)) <= first.final_deadline);
    }
    #[test]
    fn unrepresentable_budgets_fail_closed() {
        let budget = Budgets {
            grace: Duration::MAX,
            abort_reap: Duration::MAX,
            storage_close: Duration::MAX,
            runtime: Duration::MAX,
        };
        let mut context = ShutdownContext::default();
        context.begin(StopCause::Signal, budget);
        assert!(context.plan.unwrap().invalid);
    }
}
