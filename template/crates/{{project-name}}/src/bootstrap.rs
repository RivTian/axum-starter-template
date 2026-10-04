//! Running the service: logging first, then the runtime, the signal handlers and the services,
//! and finally the exit code.

use std::io::IsTerminal;
use std::time::Duration;

use svc_runtime::prelude::*;
use svc_runtime::signal::UnixSignals;
use svc_util::prelude::*;

use crate::exit::{self, Exit};
use crate::names::SERVICE_NAME;
use crate::settings::Config;
use crate::wiring;

/// Runs the service until a signal or a fault ends it.
pub(crate) fn run() -> Exit {
    let config = Config::default();
    if let Err(error) =
        svc_telemetry::subscriber::init(&config.log, std::io::stdout().is_terminal())
    {
        exit::report(&format!("cannot start logging: {}", error.fields().context));
        return Exit::CANT_CREATE;
    }
    let version = env!("CARGO_PKG_VERSION");
    tracing::info!(
        service.name = SERVICE_NAME,
        service.version = version,
        "starting"
    );
    let code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => {
            let code = runtime.block_on(serve(&config));
            // The supervisor has stopped or aborted every service it started.
            runtime.shutdown_timeout(Duration::from_millis(500));
            code
        }
        Err(error) => {
            tracing::error!(error.message = %error, "cannot build the runtime");
            Exit::OS_ERROR
        }
    };
    tracing::info!(exit_code = code.value(), "stopped");
    code
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
