//! Signals: the signals the supervisor reacts to, where they come from, and the
//! handlers for the real ones. SIGQUIT, SIGUSR1 and the rest keep the operating system's
//! default behaviour.

use std::future::Future;

use svc_util::prelude::*;

/// A signal the supervisor reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// SIGTERM: drain after `lifecycle.drain_delay`, then stop.
    Terminate,
    /// SIGINT: drain at once, then stop.
    Interrupt,
    /// SIGHUP: logged and ignored; reloading is not supported.
    Hangup,
}

impl Signal {
    /// The signal's name, such as `SIGTERM`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Signal::Terminate => "SIGTERM",
            Signal::Interrupt => "SIGINT",
            Signal::Hangup => "SIGHUP",
        }
    }

    /// The signal's number, from which a process stopped by it takes its exit code
    /// (128 + number).
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Signal::Terminate => 15,
            Signal::Interrupt => 2,
            Signal::Hangup => 1,
        }
    }
}

/// Where the supervisor gets signals from: the real handlers, or a fake in tests.
pub trait SignalSource: Send + 'static {
    /// Waits for the next signal. Dropping the future before it completes loses no signal.
    fn next(&mut self) -> impl Future<Output = Signal> + Send;
}

/// Handlers for SIGTERM, SIGINT and SIGHUP. Unix only: signals work differently on
/// Windows, where this template is not verified.
#[cfg(unix)]
#[derive(Debug)]
pub struct UnixSignals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl UnixSignals {
    /// Installs the handlers. Call it first thing inside the runtime; until then the
    /// signals keep their default behaviour.
    ///
    /// # Errors
    ///
    /// [`SIGNAL_SETUP_FAILED`](crate::error::SIGNAL_SETUP_FAILED) when a handler cannot be
    /// installed.
    pub fn install() -> Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};

        use crate::error::SIGNAL_SETUP_FAILED;

        let handler = |kind: SignalKind, name: &'static str| {
            signal(kind).or_err_with(SIGNAL_SETUP_FAILED, || format!("cannot handle {name}"))
        };
        Ok(UnixSignals {
            terminate: handler(SignalKind::terminate(), "SIGTERM")?,
            interrupt: handler(SignalKind::interrupt(), "SIGINT")?,
            hangup: handler(SignalKind::hangup(), "SIGHUP")?,
        })
    }
}

#[cfg(unix)]
impl SignalSource for UnixSignals {
    async fn next(&mut self) -> Signal {
        // A handler's stream never ends while it is installed, so `None` does not occur.
        tokio::select! {
            _ = self.terminate.recv() => Signal::Terminate,
            _ = self.interrupt.recv() => Signal::Interrupt,
            _ = self.hangup.recv() => Signal::Hangup,
        }
    }
}
