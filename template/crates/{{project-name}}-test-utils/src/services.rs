//! A service that follows a script, and records what happened to it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use svc_runtime::service::{Service, ServiceContext, ServiceKind};
use svc_util::prelude::*;

/// One step of a [`ScriptedService`]'s script.
#[derive(Clone, Debug)]
pub enum Step {
    /// Sleeps.
    Sleep(Duration),
    /// Declares the service ready.
    Ready,
    /// Waits until the supervisor asks the service to stop.
    UntilShutdown,
    /// Returns `Ok(())`.
    Return,
    /// Returns an error of this type.
    Fail(ErrorType),
    /// Panics.
    Panic,
}

/// What a [`ScriptedService`] did, in order: `started`, `ready`, `shutdown requested`,
/// then `returned`, `failed` or `panicking`.
#[derive(Clone, Debug, Default)]
pub struct Journal(Arc<Mutex<Vec<&'static str>>>);

impl Journal {
    fn push(&self, entry: &'static str) {
        // Pushing cannot leave the list half changed: recover from a poisoned lock.
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

    /// The entries so far.
    #[must_use]
    pub fn entries(&self) -> Vec<&'static str> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A service that runs its script step by step; a script that ends without returning
/// returns `Ok(())`.
#[derive(Debug)]
pub struct ScriptedService {
    name: &'static str,
    kind: ServiceKind,
    steps: Vec<Step>,
    journal: Journal,
}

impl ScriptedService {
    /// A service with a name, a kind and a script.
    #[must_use]
    pub fn new(name: &'static str, kind: ServiceKind, steps: Vec<Step>) -> Self {
        ScriptedService {
            name,
            kind,
            steps,
            journal: Journal::default(),
        }
    }

    /// The record of what the service did; clones share it.
    #[must_use]
    pub fn journal(&self) -> Journal {
        self.journal.clone()
    }
}

#[async_trait]
impl Service for ScriptedService {
    fn name(&self) -> &'static str {
        self.name
    }

    fn kind(&self) -> ServiceKind {
        self.kind
    }

    async fn run(self: Box<Self>, ctx: ServiceContext) -> Result<()> {
        self.journal.push("started");
        for step in &self.steps {
            match step {
                Step::Sleep(duration) => tokio::time::sleep(*duration).await,
                Step::Ready => {
                    ctx.ready();
                    self.journal.push("ready");
                }
                Step::UntilShutdown => {
                    ctx.shutdown().await;
                    self.journal.push("shutdown requested");
                }
                Step::Return => break,
                Step::Fail(etype) => {
                    self.journal.push("failed");
                    return Error::e_explain(etype.clone(), "scripted failure");
                }
                Step::Panic => {
                    self.journal.push("panicking");
                    // Unwinds like a panic, which the supervisor must survive.
                    std::panic::resume_unwind(Box::new("scripted panic"));
                }
            }
        }
        self.journal.push("returned");
        Ok(())
    }
}
