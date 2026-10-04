//! The service binary and its composition root: the command line, the root configuration,
//! the wiring of the services, and the exit code. The binary in `main.rs` only calls
//! [`main`]; the modules live in this library, documented and linted like every other
//! crate's.

mod bootstrap;
mod build_info;
mod cli;
mod exit;
mod names;
mod probe;
mod settings;
mod wiring;

use std::process::ExitCode;

use crate::cli::Command;

/// Runs the command given on the command line and returns the exit code of the process.
#[must_use]
pub fn main() -> ExitCode {
    // First, so that a panic is visible however early it happens.
    svc_telemetry::panic::install();
    let exit = match cli::parse() {
        Ok(cli) => match cli.command {
            Command::Run(args) => bootstrap::run(&args),
            Command::CheckConfig(args) => bootstrap::check_config(&args),
            Command::Probe(args) => probe::run(&args),
        },
        Err(exit) => exit,
    };
    exit.into()
}
