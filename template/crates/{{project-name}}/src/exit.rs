//! Exit codes, from sysexits, and the only places that write to stdout and stderr directly.

use std::io::Write;
use std::process::ExitCode;

use svc_runtime::supervisor::Outcome;
use svc_util::prelude::*;

use crate::names::SERVICE_NAME;

/// The exit code of the process. Only the values defined here exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Exit(u8);

impl Exit {
    /// Stopped after a signal; `check-config` passed; help or version.
    pub(crate) const OK: Exit = Exit(0);
    /// `probe` only: no 2xx answer.
    pub(crate) const PROBE_FAILED: Exit = Exit(1);
    /// The command line is wrong (`EX_USAGE`).
    pub(crate) const USAGE: Exit = Exit(64);
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
    /// The configuration is invalid (`EX_CONFIG`).
    pub(crate) const CONFIG: Exit = Exit(78);

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

/// Why a run ended badly, in one line for stderr, so that the reason is seen whatever the log
/// filter and wherever the logs go; nothing after a clean stop.
pub(crate) fn why(outcome: &Outcome) -> Option<String> {
    let reason = |error: &Error| {
        let fields = error.fields();
        let said = if fields.context.is_empty() {
            fields.etype
        } else {
            fields.context.as_str()
        };
        if fields.cause.is_empty() {
            said.to_string()
        } else {
            format!("{said}: {}", fields.cause)
        }
    };
    match outcome {
        Outcome::Stopped => None,
        Outcome::StartupFailed(error) => Some(format!("startup failed: {}", reason(error))),
        Outcome::Fault(error) => Some(format!("stopped after a fault: {}", reason(error))),
        Outcome::DrainTimedOut => {
            Some("services were still running at the shutdown deadline".to_string())
        }
        Outcome::Aborted(signal) => Some(format!(
            "the shutdown was cut short by a second signal ({})",
            signal.name()
        )),
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.0)
    }
}

/// Writes text to stdout. A closed stdout, as with `| head -0`, is ignored: nobody reads it.
pub(crate) fn print(text: &str) {
    std::io::stdout().lock().write_all(text.as_bytes()).ok();
}

/// Writes each line of a message to stderr after the service name, for what happens before
/// logging is up and for why a run ended badly. A closed stderr is ignored: there is nowhere
/// else to say it.
pub(crate) fn report(message: &str) {
    let mut stderr = std::io::stderr().lock();
    for line in message.lines() {
        writeln!(stderr, "{SERVICE_NAME}: {line}").ok();
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use svc_runtime::signal::Signal;
    use svc_runtime::supervisor::Outcome;
    use svc_util::prelude::*;

    use super::{Exit, why};

    #[test]
    fn each_outcome_has_its_code_and_each_bad_one_a_reason() {
        let failed = Error::because(
            ErrorType::InternalError,
            "service http failed",
            io::Error::other("address in use"),
        );
        let cases = [
            (Outcome::Stopped, 0, None),
            (
                Outcome::StartupFailed(failed),
                69,
                Some("startup failed: service http failed: address in use"),
            ),
            (
                Outcome::Fault(Error::explain(
                    ErrorType::InternalError,
                    "service http returned early",
                )),
                70,
                Some("stopped after a fault: service http returned early"),
            ),
            (
                Outcome::DrainTimedOut,
                75,
                Some("services were still running at the shutdown deadline"),
            ),
            (
                Outcome::Aborted(Signal::Interrupt),
                130,
                Some("the shutdown was cut short by a second signal (SIGINT)"),
            ),
            (
                Outcome::Aborted(Signal::Terminate),
                143,
                Some("the shutdown was cut short by a second signal (SIGTERM)"),
            ),
        ];
        for (outcome, code, reason) in cases {
            assert_eq!(Exit::of(&outcome).value(), code, "{}", outcome.name());
            assert_eq!(why(&outcome).as_deref(), reason, "{}", outcome.name());
        }
    }
}
