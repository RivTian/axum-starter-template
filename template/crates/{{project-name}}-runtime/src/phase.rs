//! The lifecycle phase of the process. Only the supervisor moves it forward; anyone may read
//! it or wait for a change.

use std::fmt;
use std::sync::Arc;

use tokio::sync::watch;

/// A lifecycle phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Services are starting.
    Starting,
    /// Every service is ready.
    Running,
    /// SIGTERM arrived: readiness reports 503 while requests are still served.
    Draining,
    /// Services are asked to stop: frontline services first, then background services.
    Stopping,
    /// Every service has stopped, or the shutdown was cut short.
    Stopped,
}

impl Phase {
    /// The phase in lower case, as logged and as `/readyz` reports it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::Draining => "draining",
            Phase::Stopping => "stopping",
            Phase::Stopped => "stopped",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The current phase. Clones share it.
#[derive(Clone, Debug)]
pub struct PhaseWatch {
    sender: Arc<watch::Sender<Phase>>,
}

impl PhaseWatch {
    /// A watch in [`Phase::Starting`].
    #[must_use]
    pub fn new() -> Self {
        PhaseWatch {
            sender: Arc::new(watch::Sender::new(Phase::Starting)),
        }
    }

    /// The current phase.
    #[must_use]
    pub fn current(&self) -> Phase {
        *self.sender.borrow()
    }

    /// A receiver that sees the changes from now on.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Phase> {
        self.sender.subscribe()
    }

    pub(crate) fn set(&self, phase: Phase) {
        self.sender.send_replace(phase);
    }
}

impl Default for PhaseWatch {
    fn default() -> Self {
        Self::new()
    }
}
