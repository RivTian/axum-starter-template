//! Where the effective value of every key came from.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// The source of a key's effective value, shown as `default`, `file <path>`,
/// `env <NAME>` or `cli --<flag>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The schema's default.
    Default,
    /// The configuration file at this path.
    File(PathBuf),
    /// This environment variable, such as `RUST_LOG`.
    Env(String),
    /// This command-line flag, such as `--http-addr`.
    Cli(&'static str),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("default"),
            Source::File(path) => write!(f, "file {}", path.display()),
            Source::Env(name) => write!(f, "env {name}"),
            Source::Cli(flag) => write!(f, "cli {flag}"),
        }
    }
}

/// The source of every key of the schema, in key order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    by_key: BTreeMap<String, Source>,
}

impl Sources {
    /// The source of a key. Keys outside the schema count as [`Source::Default`].
    #[must_use]
    pub fn of(&self, key: &str) -> &Source {
        self.by_key.get(key).unwrap_or(&Source::Default)
    }

    /// Every key and its source, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Source)> {
        self.by_key
            .iter()
            .map(|(key, source)| (key.as_str(), source))
    }

    pub(crate) fn set(&mut self, key: &str, source: Source) {
        self.by_key.insert(key.to_string(), source);
    }
}
