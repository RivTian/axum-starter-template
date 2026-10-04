//! Exit codes, from sysexits, and the one place that writes to stderr.

use std::io::Write;
use std::process::ExitCode;

use svc_runtime::supervisor::Outcome;

use crate::names::SERVICE_NAME;

/// The exit code of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Exit(u8);

impl Exit {
    /// Stopped after a signal.
    pub(crate) const OK: Exit = Exit(0);
    /// The service could not start (`EX_UNAVAILABLE`).
    pub(crate) const STARTUP_FAILED: Exit = Exit(69);
    /// A fault while running (`EX_SOFTWARE`).
    pub(crate) const FAULT: Exit = Exit(70);
    /// The runtime or the signal handlers could not be set up (`EX_OSERR`).
    pub(crate) const OS_ERROR: Exit = Exit(71);
    /// Logging could not start (`EX_CANTCREAT`).
    pub(crate) const CANT_CREATE: Exit = Exit(73);
    /// Requests were still running at the shutdown deadline (`EX_TEMPFAIL`).
    pub(crate) const DRAIN_TIMED_OUT: Exit = Exit(75);

    /// The code as a number, as logged.
    pub(crate) const fn value(self) -> u8 {
        self.0
    }

    /// The exit code for how the supervisor's run ended.
    pub(crate) fn of(outcome: &Outcome) -> Exit {
        match outcome {
            Outcome::Stopped => Exit::OK,
            Outcome::StartupFailed(_) => Exit::STARTUP_FAILED,
            Outcome::Fault(_) => Exit::FAULT,
            Outcome::DrainTimedOut => Exit::DRAIN_TIMED_OUT,
            // 128 plus the signal's number, as a shell reports a process the signal stopped.
            Outcome::Aborted(signal) => Exit(128 + signal.number()),
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.0)
    }
}

/// Writes one line to stderr, for what happens before logging is up. A closed stderr is
/// ignored: there is nowhere else to say it.
pub(crate) fn report(message: &str) {
    writeln!(std::io::stderr().lock(), "{SERVICE_NAME}: {message}").ok();
}
