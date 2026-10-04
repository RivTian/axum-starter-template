//! The service binary and its composition root: the root configuration, the wiring of the
//! services, and the exit code. The binary in `main.rs` only calls [`main`]; the logic lives in
//! this library so that tests can reach it.

mod bootstrap;
mod exit;
mod names;
mod settings;
mod wiring;

use std::process::ExitCode;

/// Runs the service and returns the exit code of the process.
#[must_use]
pub fn main() -> ExitCode {
    bootstrap::run().into()
}
