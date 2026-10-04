//! The configuration section logging owns.

/// `[log]`: logging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSettings {
    /// `log.format`: the format of the lines on stdout.
    pub format: LogFormat,
    /// `log.filter`: which events to log, in `EnvFilter` syntax.
    pub filter: String,
}

impl Default for LogSettings {
    fn default() -> Self {
        LogSettings {
            format: LogFormat::Auto,
            filter: "info".to_string(),
        }
    }
}

/// The format of log lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    /// Text on a terminal, JSON otherwise.
    Auto,
    /// Human-readable text.
    Text,
    /// One JSON object per line.
    Json,
}
