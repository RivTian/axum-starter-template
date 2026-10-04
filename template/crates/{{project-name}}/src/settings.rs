//! The root configuration: one section per crate that owns one.

use svc_api::settings::ServerSettings;
use svc_runtime::settings::{EventSettings, LifecycleSettings};
use svc_telemetry::settings::LogSettings;

/// The whole configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Config {
    /// `[server]`: the HTTP server.
    pub(crate) server: ServerSettings,
    /// `[lifecycle]`: start-up and shutdown timing.
    pub(crate) lifecycle: LifecycleSettings,
    /// `[events]`: the in-process event bus.
    pub(crate) events: EventSettings,
    /// `[log]`: logging.
    pub(crate) log: LogSettings,
}
