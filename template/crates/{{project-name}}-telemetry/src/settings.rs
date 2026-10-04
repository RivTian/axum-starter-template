//! The configuration section logging owns. The filters are checked while the configuration
//! is loaded, so a bad filter is reported with every other problem.

use std::path::PathBuf;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

/// `[log]`: logging.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSettings {
    /// `log.format`: `auto` (text on a terminal, JSON otherwise), `text` or `json`.
    #[serde(deserialize_with = "format")]
    pub format: LogFormat,
    /// `log.filter`: which events go to stdout, in `EnvFilter` syntax; not empty.
    #[serde(deserialize_with = "filter")]
    pub filter: String,
    /// `log.color`: `auto` (on a terminal, unless `NO_COLOR` is set or `TERM` is `dumb`),
    /// `always` or `never`.
    #[serde(deserialize_with = "color")]
    pub color: LogColor,
    /// `[log.file]`: the optional log file.
    pub file: FileSettings,
}

impl Default for LogSettings {
    fn default() -> Self {
        LogSettings {
            format: LogFormat::Auto,
            filter: "info".to_string(),
            color: LogColor::Auto,
            file: FileSettings::default(),
        }
    }
}

/// `[log.file]`: the log file, rolled daily into compressed archives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSettings {
    /// `log.file.enabled`: whether to write the file.
    #[serde(deserialize_with = "svc_util::de::boolean")]
    pub enabled: bool,
    /// `log.file.dir`: the directory of `<service>.log` and its archives.
    #[serde(deserialize_with = "svc_util::de::non_empty_path")]
    pub dir: PathBuf,
    /// `log.file.filter`: the file's own filter; empty means the same as `log.filter`.
    #[serde(deserialize_with = "file_filter")]
    pub filter: String,
    /// `log.file.max_archives`: how many compressed archives to keep; 1 to 365.
    #[serde(deserialize_with = "svc_util::de::integer::<_, 1, 365>")]
    pub max_archives: usize,
}

impl Default for FileSettings {
    fn default() -> Self {
        FileSettings {
            enabled: false,
            dir: PathBuf::from("logs"),
            filter: String::new(),
            max_archives: 14,
        }
    }
}

/// The format of the lines on stdout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Text on a terminal, JSON otherwise.
    Auto,
    /// Human-readable text.
    Text,
    /// One JSON object per line.
    Json,
}

/// Whether text lines on stdout are colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogColor {
    /// On a terminal, unless `NO_COLOR` is set or `TERM` is `dumb`.
    Auto,
    /// Always.
    Always,
    /// Never.
    Never,
}

fn format<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LogFormat, D::Error> {
    const CHOICES: &[(&str, LogFormat)] = &[
        ("auto", LogFormat::Auto),
        ("text", LogFormat::Text),
        ("json", LogFormat::Json),
    ];
    svc_util::de::one_of(deserializer, CHOICES)
}

fn color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LogColor, D::Error> {
    const CHOICES: &[(&str, LogColor)] = &[
        ("auto", LogColor::Auto),
        ("always", LogColor::Always),
        ("never", LogColor::Never),
    ];
    svc_util::de::one_of(deserializer, CHOICES)
}

/// A filter that is not empty and parses; `off` turns the output off.
fn filter<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = svc_util::de::non_empty_text(deserializer)?;
    check(&text).map(|()| text)
}

/// Like [`filter`], except that empty means "the same as `log.filter`".
fn file_filter<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        Ok(text)
    } else {
        check(&text).map(|()| text)
    }
}

fn check<E: de::Error>(text: &str) -> Result<(), E> {
    EnvFilter::try_new(text)
        .map(drop)
        .map_err(|error| E::custom(format!("not a valid log filter: {error}")))
}
