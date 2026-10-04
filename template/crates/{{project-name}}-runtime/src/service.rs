//! The service contract: what a service is, and what the supervisor gives it to run.

use std::future::Future;

use async_trait::async_trait;
use svc_util::prelude::*;
use tokio::sync::{mpsc, watch};

use crate::supervisor::Input;

/// A unit of work the supervisor starts, watches and stops: the HTTP server, an event
/// subscriber, a scheduled job. Crates above the runtime implement it; the binary hands it to
/// the supervisor.
#[async_trait]
pub trait Service: Send + 'static {
    /// A short name, used in logs.
    fn name(&self) -> &'static str;

    /// Whether the service takes traffic from outside ([`ServiceKind::Frontline`]) or works
    /// in the background.
    fn kind(&self) -> ServiceKind {
        ServiceKind::Background
    }

    /// Runs the service: call `ctx.ready()` once it can do its work, and return when
    /// `ctx.shutdown()` completes.
    ///
    /// # Errors
    ///
    /// Whatever made the service fail. While the process starts this fails the startup;
    /// later it stops the process as a fault.
    async fn run(self: Box<Self>, ctx: ServiceContext) -> Result<()>;
}

/// How the supervisor treats a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// Takes traffic from outside. Returning before it is asked to stop is a fault; it is
    /// asked to stop first.
    Frontline,
    /// Works in the background. Returning `Ok` once ready is fine; it is asked to stop once
    /// the frontline services have stopped.
    Background,
}

/// What a running service receives from the supervisor.
#[derive(Debug)]
pub struct ServiceContext {
    pub(crate) index: usize,
    pub(crate) events: mpsc::UnboundedSender<Input>,
    pub(crate) shutdown: watch::Receiver<bool>,
}

impl ServiceContext {
    /// Declares the service ready. Calling it again has no effect.
    pub fn ready(&self) {
        // The supervisor outlives every service, so the channel is open.
        self.events.send(Input::Ready(self.index)).ok();
    }

    /// Completes when it is this service's turn to stop. The future owns what it needs, so it
    /// can be handed to a server's graceful shutdown.
    pub fn shutdown(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut shutdown = self.shutdown.clone();
        async move {
            // An error means the supervisor is gone, which is a reason to stop as well.
            shutdown.wait_for(|stop| *stop).await.ok();
        }
    }
}
