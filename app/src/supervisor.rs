//! Actual futures are owned by one JoinSet, never watcher-of-JoinHandle tasks.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::future::Future;
use tokio::runtime::Handle;
use tokio::task::{Id, JoinError, JoinSet};

// An app-local carrier, not a cross-crate AppError or an HTTP error mapper.
pub(crate) type TaskError = Box<dyn Error + Send + Sync>;
pub(crate) type TaskResult = Result<(), TaskError>;

#[derive(Debug)]
pub(crate) enum ExitKind {
    Returned,
    Failed(TaskError),
    Panicked,
    Cancelled,
    Invariant,
}
impl ExitKind {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Returned => "returned",
            Self::Failed(_) => "failed",
            Self::Panicked => "panicked",
            Self::Cancelled => "cancelled",
            Self::Invariant => "invariant",
        }
    }
    pub(crate) fn returned(&self) -> bool {
        matches!(self, Self::Returned)
    }
}
#[derive(Debug)]
pub(crate) struct TaskExit {
    pub(crate) name: &'static str,
    pub(crate) runtime: Cow<'static, str>,
    pub(crate) kind: ExitKind,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegisterError {
    #[error("duplicate task name: {0}")]
    Duplicate(&'static str),
    #[error("task registration is closed")]
    Closed,
}

pub(crate) struct TaskSupervisor {
    tasks: JoinSet<TaskResult>,
    metadata: HashMap<Id, (&'static str, Cow<'static, str>)>,
    registered: HashSet<&'static str>,
    closed: bool,
}
impl TaskSupervisor {
    pub(crate) fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            metadata: HashMap::new(),
            registered: HashSet::new(),
            closed: false,
        }
    }
    pub(crate) fn spawn_on<F>(
        &mut self,
        name: &'static str,
        runtime: &Handle,
        runtime_name: impl Into<Cow<'static, str>>,
        future: F,
    ) -> Result<(), RegisterError>
    where
        F: Future<Output = TaskResult> + Send + 'static,
    {
        if self.closed {
            return Err(RegisterError::Closed);
        }
        if !self.registered.insert(name) {
            return Err(RegisterError::Duplicate(name));
        }
        let abort = self.tasks.spawn_on(future, runtime);
        self.metadata
            .insert(abort.id(), (name, runtime_name.into()));
        Ok(())
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
    pub(crate) fn len(&self) -> usize {
        self.tasks.len()
    }
    pub(crate) fn close_registration(&mut self) {
        self.closed = true;
    }
    pub(crate) fn abort_remaining(&mut self) {
        self.tasks.abort_all();
    }
    pub(crate) fn pending_names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.metadata.values().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names
    }
    pub(crate) async fn next_exit(&mut self) -> Option<TaskExit> {
        let exit = self.tasks.join_next_with_id().await?;
        Some(self.record(exit))
    }
    pub(crate) fn try_next_exit(&mut self) -> Option<TaskExit> {
        let exit = self.tasks.try_join_next_with_id()?;
        Some(self.record(exit))
    }
    fn record(&mut self, result: Result<(Id, TaskResult), JoinError>) -> TaskExit {
        let (id, kind) = match result {
            Ok((id, Ok(()))) => (id, ExitKind::Returned),
            Ok((id, Err(error))) => (id, ExitKind::Failed(error)),
            Err(error) => (
                error.id(),
                if error.is_panic() {
                    ExitKind::Panicked
                } else {
                    ExitKind::Cancelled
                },
            ),
        };
        match self.metadata.remove(&id) {
            Some((name, runtime)) => TaskExit {
                name,
                runtime,
                kind,
            },
            None => TaskExit {
                name: "<unknown>",
                runtime: Cow::Borrowed("<unknown>"),
                kind: ExitKind::Invariant,
            },
        }
    }
}
// JoinSet's Drop requests abort. Only the shutdown collector can confirm joins.

#[cfg(test)]
#[path = "supervisor/tests.rs"]
mod tests;
