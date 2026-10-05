//! Signals: the signals the supervisor reacts to, where they come from, and the
//! handlers for the real ones, on Unix and on Windows. On Unix, SIGQUIT, SIGUSR1 and the
//! rest keep the operating system's default behaviour; on Windows, the console events stand
//! for the signals, so the supervisor, the logs and the exit codes are the same on both.

use std::future::Future;

use svc_util::prelude::*;

#[cfg(not(any(unix, windows)))]
compile_error!("signal handling is written for Unix and Windows only");

/// A signal the supervisor reacts to. On Windows the console events count as these (see
/// [`OsSignals`]) and keep their names and numbers.
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

/// The operating system's handlers.
///
/// | Unix    | Windows                                  | Counts as |
/// | ------- | ---------------------------------------- | --------- |
/// | SIGTERM | `CTRL_CLOSE`, `CTRL_LOGOFF`, `CTRL_SHUTDOWN` | SIGTERM   |
/// | SIGINT  | `CTRL_C`, `CTRL_BREAK`                   | SIGINT    |
/// | SIGHUP  | (none)                                   | SIGHUP    |
///
/// `CTRL_CLOSE` comes when the console window is closed, `CTRL_SHUTDOWN` when the system
/// shuts down and with `docker stop` in a Windows container. Windows ends the process a few
/// seconds after `CTRL_CLOSE`, `CTRL_LOGOFF` or `CTRL_SHUTDOWN` whatever it does, so keep
/// `drain_delay` plus `drain_timeout` short there.
#[derive(Debug)]
pub struct OsSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hangup: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    break_key: tokio::signal::windows::CtrlBreak,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    logoff: tokio::signal::windows::CtrlLogoff,
    #[cfg(windows)]
    shutdown: tokio::signal::windows::CtrlShutdown,
}

impl OsSignals {
    /// Installs the handlers. Call it first thing inside the runtime; until then the
    /// signals keep their default behaviour.
    ///
    /// # Errors
    ///
    /// [`SIGNAL_SETUP_FAILED`](crate::error::SIGNAL_SETUP_FAILED) when a handler cannot be
    /// installed.
    pub fn install() -> Result<Self> {
        use crate::error::SIGNAL_SETUP_FAILED;

        let failed = |name: &'static str| move || format!("cannot handle {name}");
        #[cfg(unix)]
        let signals = {
            use tokio::signal::unix::{SignalKind, signal};

            let handler = |kind: SignalKind, name: &'static str| {
                signal(kind).or_err_with(SIGNAL_SETUP_FAILED, failed(name))
            };
            OsSignals {
                terminate: handler(SignalKind::terminate(), "SIGTERM")?,
                interrupt: handler(SignalKind::interrupt(), "SIGINT")?,
                hangup: handler(SignalKind::hangup(), "SIGHUP")?,
            }
        };
        #[cfg(windows)]
        let signals = {
            use tokio::signal::windows;

            OsSignals {
                interrupt: windows::ctrl_c().or_err_with(SIGNAL_SETUP_FAILED, failed("CTRL_C"))?,
                break_key: windows::ctrl_break()
                    .or_err_with(SIGNAL_SETUP_FAILED, failed("CTRL_BREAK"))?,
                close: windows::ctrl_close()
                    .or_err_with(SIGNAL_SETUP_FAILED, failed("CTRL_CLOSE"))?,
                logoff: windows::ctrl_logoff()
                    .or_err_with(SIGNAL_SETUP_FAILED, failed("CTRL_LOGOFF"))?,
                shutdown: windows::ctrl_shutdown()
                    .or_err_with(SIGNAL_SETUP_FAILED, failed("CTRL_SHUTDOWN"))?,
            }
        };
        Ok(signals)
    }
}

impl SignalSource for OsSignals {
    async fn next(&mut self) -> Signal {
        // A handler's stream never ends while it is installed, so `None` does not occur.
        #[cfg(unix)]
        let signal = tokio::select! {
            _ = self.terminate.recv() => Signal::Terminate,
            _ = self.interrupt.recv() => Signal::Interrupt,
            _ = self.hangup.recv() => Signal::Hangup,
        };
        #[cfg(windows)]
        let signal = tokio::select! {
            _ = self.interrupt.recv() => Signal::Interrupt,
            _ = self.break_key.recv() => Signal::Interrupt,
            _ = self.close.recv() => Signal::Terminate,
            _ = self.logoff.recv() => Signal::Terminate,
            _ = self.shutdown.recv() => Signal::Terminate,
        };
        signal
    }
}
