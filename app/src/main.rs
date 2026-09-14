//! Synchronous composition root: configuration precedes the runtime, and runtime
//! destruction happens outside block_on, including coordinator unwinding.

mod boot;
mod cli;
mod config;
mod rt;
mod shutdown;
mod signals;
mod supervisor;
mod telemetry;

use service_core::BuildInfo;
use shutdown::{RunReport, ShutdownContext, StopCause};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let build = BuildInfo {
        service: env!("CARGO_BIN_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    };
    let action = match cli::parse(std::env::args_os().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let explicit = match action {
        cli::Action::Version => {
            println!("{} {}", build.service, build.version);
            return ExitCode::SUCCESS;
        }
        cli::Action::Help => {
            println!(
                "{}\n  --version\n  --help\n  --config <path>\nDefault: {}; alternatively {}_CONFIG.\nDefault: one runtime; optional task bindings are cold configuration. SIGHUP reloads only ticker.interval_ms.",
                build.service,
                config::DEFAULT_PATH,
                build.service.to_ascii_uppercase()
            );
            return ExitCode::SUCCESS;
        }
        cli::Action::Run { config } => config,
    };
    let environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let key = format!("{}_CONFIG", build.service.to_ascii_uppercase());
    let (path, config_source) =
        config::select_path(explicit, &environment, std::ffi::OsStr::new(&key));
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("cannot resolve the current directory");
            return ExitCode::FAILURE;
        }
    };
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    // Retain the same path, environment and resolved worker-count snapshot for
    // every reload; no new reads of process environment/cwd/CPU discovery.
    let load_context = match config::LoadContext::new(path, &cwd, environment, workers) {
        Ok(context) => Arc::new(context),
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let config = match config::load(&load_context) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = telemetry::init() {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    tracing::info!(
        service = build.service,
        version = build.version,
        config_source,
        "starting service"
    );
    let mut runtime = match rt::RuntimeSet::build(&config.boot) {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(error = %error, "runtime topology construction failed");
            return ExitCode::FAILURE;
        }
    };
    let executors = runtime.executors();
    let mut context = ShutdownContext::default();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(async {
        // Subscriptions are installed before any database or task startup.
        let mut signals = signals::Signals::install()?;
        Ok::<_, std::io::Error>(boot::run(&config, build, &mut context, async || signals.next().await, |address| {
            tracing::info!(event = "service_started", listen_addr = %address, "service running");
        }, move || config::load(&load_context), &executors).await)
    })
    }));
    let mut report = match outcome {
        Ok(Ok(report)) => report,
        Ok(Err(source)) => {
            context.begin(
                StopCause::Startup {
                    phase: "signal_registration",
                    source: Box::new(source),
                },
                config.boot.shutdown,
            );
            RunReport::failed(context.cause.take().expect("cause recorded"))
        }
        Err(_) => {
            context.begin(StopCause::CoordinatorPanic, config.boot.shutdown);
            let mut report = RunReport::failed(context.cause.take().expect("cause recorded"));
            report.unreaped = std::mem::take(&mut context.emergency_unreaped);
            report
        }
    };
    let exhausted = runtime.shutdown(context.runtime_deadline(config.boot.shutdown.runtime));
    report.cleanup_failed |= exhausted;
    let elapsed = context
        .plan
        .map_or(std::time::Duration::ZERO, |plan| plan.started.elapsed());
    telemetry::report(&report, elapsed);
    if report.succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
