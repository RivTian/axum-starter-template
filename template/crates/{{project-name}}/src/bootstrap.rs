//! What `check-config` and `run` do. Both load the configuration the same way, so the same
//! input gets the same verdict; `run` then starts logging, the runtime, the signal handlers
//! and the services.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use svc_config::load::{Inputs, Loaded, load};
use svc_config::table::{overrides, render, rows};
use svc_runtime::prelude::*;
use svc_runtime::signal::UnixSignals;
use svc_telemetry::subscriber::Environment;
use svc_util::prelude::*;

use crate::build_info;
use crate::cli::ConfigArgs;
use crate::exit::{self, Exit};
use crate::names::{ENV_PREFIX, SERVICE_NAME};
use crate::settings::{ALIASES, Config};
use crate::wiring;

/// `check-config`: prints every key with its value and source, or the problems.
pub(crate) fn check_config(args: &ConfigArgs) -> Exit {
    match configuration(args) {
        Ok(loaded) => {
            exit::print(&format!(
                "configuration is valid\n{}",
                render(&rows(&loaded))
            ));
            Exit::OK
        }
        Err(exit) => exit,
    }
}

/// `run`: runs the service until a signal or a fault ends it.
pub(crate) fn run(args: &ConfigArgs) -> Exit {
    let loaded = match configuration(args) {
        Ok(loaded) => loaded,
        Err(exit) => return exit,
    };
    let config = &loaded.config;
    let env = environment();
    let guard = match svc_telemetry::subscriber::init(&config.log, &env) {
        Ok(guard) => guard,
        Err(error) => {
            exit::report(&format!("cannot start logging: {}", error.fields().context));
            return Exit::CANT_CREATE;
        }
    };
    let (version, sha) = (build_info::VERSION, build_info::git_sha());
    tracing::info!(
        service.name = SERVICE_NAME,
        service.version = version,
        git_sha = sha,
        "starting"
    );
    tracing::info!(overrides = overrides(&rows(&loaded)), "configuration");
    let code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => {
            let code = runtime.block_on(serve(config));
            // The supervisor has stopped or aborted every service it started.
            runtime.shutdown_timeout(Duration::from_millis(500));
            code
        }
        Err(error) => {
            tracing::error!(error.context = %error, "cannot build the runtime");
            Exit::OS_ERROR
        }
    };
    tracing::info!(exit_code = code.value(), "stopped");
    drop(guard);
    code
}

/// Loads the configuration from `--config` or `<PREFIX>_CONFIG`, the environment and the
/// command line, or reports every problem and says to exit with 78.
fn configuration(args: &ConfigArgs) -> Result<Loaded<Config>, Exit> {
    let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let config_var = format!("{ENV_PREFIX}_CONFIG");
    let named = env.iter().find(|(name, _)| *name == *config_var);
    let file = (args.config.clone()).or_else(|| named.map(|(_, path)| PathBuf::from(path)));
    let cli = args.overrides();
    let inputs = Inputs {
        file: file.as_deref(),
        env: &env,
        prefix: ENV_PREFIX,
        aliases: ALIASES,
        cli: &cli,
    };
    load::<Config>(&inputs).map_err(|report| {
        exit::report(&report.to_string());
        Exit::CONFIG
    })
}

/// What logging needs to know about the terminal.
fn environment() -> Environment {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    Environment {
        stdout_is_terminal: std::io::stdout().is_terminal(),
        no_color: var("NO_COLOR").is_some(),
        dumb_terminal: var("TERM").is_some_and(|term| term == "dumb"),
        service_name: SERVICE_NAME,
    }
}

/// Installs the signal handlers, then runs the services.
async fn serve(config: &Config) -> Exit {
    let signals = match UnixSignals::install() {
        Ok(signals) => signals,
        Err(error) => {
            log_error!(
                tracing::Level::ERROR,
                &error,
                "cannot install the signal handlers"
            );
            return Exit::OS_ERROR;
        }
    };
    let outcome = wiring::supervisor(config, &PhaseWatch::new())
        .run(signals)
        .await;
    Exit::of(&outcome)
}
