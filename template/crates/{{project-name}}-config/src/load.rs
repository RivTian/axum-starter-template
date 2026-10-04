//! The generic loader. Sources are merged in order of priority, lowest first: the defaults,
//! the configuration file, aliases such as `RUST_LOG`, the project's environment variables
//! and the command-line overrides. The serialized defaults are the schema, so a key that is
//! not in them is rejected, from the file, the command line or a `<PREFIX>_<SECTION>__<KEY>`
//! variable. A secret (a key whose default serializes as `<redacted>`) never shows its value.

mod env;
mod file;
mod keys;
mod layered;

use std::ffi::OsString;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use toml::Value;

use crate::report::{Problem, Report};
use crate::source::Sources;
use layered::Layered;

/// What [`load`] reads. The binary fills it from the command line and the process
/// environment; tests pass their own values.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The configuration file given with `--config` or `<PREFIX>_CONFIG`, if any; no other
    /// file is looked for.
    pub file: Option<&'a Path>,
    /// Environment variables, as the operating system gives them. Aliases and the ones named
    /// `<PREFIX>_<SECTION>__<KEY>` are read; the rest are ignored, including prefixed names
    /// without `__`, such as the ones Kubernetes adds for a Service of the same name. A value
    /// that is not UTF-8 is a problem if it takes effect.
    pub env: &'a [(OsString, OsString)],
    /// The environment variable prefix: the project name in upper snake case. With `APP`,
    /// `APP_LOG__FILE__ENABLED` sets `log.file.enabled`.
    pub prefix: &'a str,
    /// Variables that set one key, below the prefixed variables, as (key, variable), such as
    /// `("log.filter", "RUST_LOG")`. An empty value counts as unset.
    pub aliases: &'a [(&'static str, &'static str)],
    /// Command-line overrides as (key, flag, value), such as
    /// `("server.http_addr", "--http-addr", "0.0.0.0:8080")`.
    pub cli: &'a [(&'static str, &'static str, String)],
}

/// A loaded configuration and the source of every key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded<T> {
    /// The configuration.
    pub config: T,
    /// Where the effective value of each key came from.
    pub sources: Sources,
}

/// Loads the configuration of schema `T` from the inputs.
///
/// # Errors
///
/// Every problem found: unknown keys, a section given a value, a file that cannot be read or
/// is not valid TOML, environment values that are not UTF-8, and values of the wrong form or
/// out of range. Values are checked even when other problems were found.
pub fn load<T: Serialize + DeserializeOwned + Default>(
    inputs: &Inputs<'_>,
) -> Result<Loaded<T>, Report> {
    let Ok(Value::Table(defaults)) = Value::try_from(T::default()) else {
        let detail = "the configuration schema does not serialize as a table";
        return Err(Report::new(vec![Problem::whole(detail)]));
    };
    let mut tree = Layered::new(defaults);
    if let Some(path) = inputs.file {
        tree.merge_file(path);
    }
    tree.merge_aliases(inputs.aliases, inputs.env);
    tree.merge_env(inputs.prefix, inputs.env);
    tree.merge_cli(inputs.cli);
    tree.report_not_utf8();
    match T::deserialize(Value::Table(tree.value.clone())) {
        Ok(config) if tree.problems.is_empty() => {
            return Ok(Loaded {
                config,
                sources: tree.sources(),
            });
        }
        Ok(_) => {}
        Err(error) => tree.locate_bad_keys::<T>(&error),
    }
    Err(Report::new(tree.problems))
}
