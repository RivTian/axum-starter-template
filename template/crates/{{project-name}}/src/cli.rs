//! The command line: `run`, `check-config` and `probe`, and the three overrides that `run`
//! and `check-config` accept.

use std::path::PathBuf;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::build_info;
use crate::exit::Exit;
use crate::names::{ENV_PREFIX, SERVICE_NAME};

/// The service's command line.
#[derive(Debug, Parser)]
#[command(name = SERVICE_NAME, version = build_info::version(), about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// What to do.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run the service until a shutdown signal.
    Run(ConfigArgs),
    /// Check the configuration and print every key's value and where it came from.
    CheckConfig(ConfigArgs),
    /// Send GET to a URL: exit 0 for a 2xx answer, 1 otherwise.
    Probe(ProbeArgs),
}

/// Where the configuration comes from, and the keys the command line overrides.
#[derive(Debug, Args)]
pub(crate) struct ConfigArgs {
    /// The configuration file; the variable `<PREFIX>_CONFIG` names one too, and the help
    /// shows its real name (see `parse`). No other file is read.
    #[arg(long, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,
    /// Override `server.http_addr`.
    #[arg(long, value_name = "ADDR")]
    pub(crate) http_addr: Option<String>,
    /// Override `log.filter`.
    #[arg(long, value_name = "FILTER")]
    pub(crate) log_filter: Option<String>,
    /// Override `log.format`.
    #[arg(long, value_name = "FORMAT")]
    pub(crate) log_format: Option<String>,
}

impl ConfigArgs {
    /// The overrides as the loader takes them: key, flag and value.
    pub(crate) fn overrides(&self) -> Vec<(&'static str, &'static str, String)> {
        [
            ("server.http_addr", "--http-addr", &self.http_addr),
            ("log.filter", "--log-filter", &self.log_filter),
            ("log.format", "--log-format", &self.log_format),
        ]
        .into_iter()
        .filter_map(|(key, flag, value)| Some((key, flag, value.clone()?)))
        .collect()
    }
}

/// The arguments of `probe`.
#[derive(Debug, Args)]
pub(crate) struct ProbeArgs {
    #[arg(
        value_name = "URL",
        help = "The URL to send GET to, such as http://127.0.0.1:8080/readyz"
    )]
    pub(crate) url: String,
}

/// Parses the command line. Help and version are printed here and end with 0; usage
/// errors are printed by clap and end with 64.
pub(crate) fn parse() -> Result<Cli, Exit> {
    // The help names the configuration variable, which depends on the project name.
    let config_help =
        format!("The configuration file; {ENV_PREFIX}_CONFIG names one too. No other file is read");
    let command = Cli::command()
        .mut_subcommand("run", |run| {
            run.mut_arg("config", |arg| arg.help(config_help.clone()))
        })
        .mut_subcommand("check-config", |check| {
            check.mut_arg("config", |arg| arg.help(config_help.clone()))
        });
    command
        .try_get_matches()
        .and_then(|matches| Cli::from_arg_matches(&matches))
        .map_err(|error| {
            let exit = if error.use_stderr() {
                Exit::USAGE
            } else {
                Exit::OK
            };
            // Printing only fails when the output is closed, and then nobody reads it.
            error.print().ok();
            exit
        })
}
