//! Installing the global subscriber.

use svc_util::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::error::{LOG_ALREADY_INITIALIZED, LOG_CONFIG_INVALID};
use crate::settings::{LogFormat, LogSettings};

/// Installs the global subscriber, writing to stdout with `log.filter`, and the bridge that
/// turns `log` records into events. `stdout_is_terminal` decides the `auto` format.
///
/// # Errors
///
/// [`LOG_CONFIG_INVALID`] when the filter does not parse; [`LOG_ALREADY_INITIALIZED`] when a
/// global subscriber is already set.
pub fn init(settings: &LogSettings, stdout_is_terminal: bool) -> Result<()> {
    let filter = EnvFilter::try_new(&settings.filter).or_err_with(LOG_CONFIG_INVALID, || {
        format!("log.filter {:?}", settings.filter)
    })?;
    let json = match settings.format {
        LogFormat::Auto => !stdout_is_terminal,
        LogFormat::Text => false,
        LogFormat::Json => true,
    };
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(stdout_is_terminal);
    let installed = if json {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
    installed.map_err(|error| Error::explain(LOG_ALREADY_INITIALIZED, error.to_string()))
}
