//! The root configuration: one section per crate that owns one. A new section is a field here
//! and a table in `config/example.toml`.

use serde::{Deserialize, Serialize};
use svc_api::settings::ServerSettings;
use svc_runtime::settings::{EventSettings, LifecycleSettings};
use svc_telemetry::settings::LogSettings;

/// Variables outside the prefix that set one key: `RUST_LOG` sets `log.filter`.
pub(crate) const ALIASES: &[(&str, &str)] = &[("log.filter", "RUST_LOG")];

/// The whole configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use svc_config::load::{Inputs, load};

    use super::{ALIASES, Config};

    /// `config/example.toml` lists every key with its default: loading it changes nothing.
    #[test]
    fn the_example_configuration_is_the_default() -> Result<(), String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/example.toml");
        let inputs = Inputs {
            file: Some(std::path::Path::new(path)),
            env: &[],
            prefix: "UNUSED",
            aliases: ALIASES,
            cli: &[],
        };
        let loaded = load::<Config>(&inputs).map_err(|report| report.to_string())?;
        assert_eq!(loaded.config, Config::default());
        let from_file = loaded
            .sources
            .iter()
            .filter(|(_, source)| source.to_string().starts_with("file "));
        assert_eq!(
            from_file.count(),
            loaded.sources.iter().count(),
            "every key is in the file"
        );
        Ok(())
    }
}
