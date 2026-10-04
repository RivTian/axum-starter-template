//! Building and installing the subscriber: one layer per output, each with its own filter,
//! and nothing filtering them all, so a strict filter on one output never starves another.

use std::fmt;

use svc_util::prelude::*;
use tracing::Subscriber;
use tracing::span::Record;
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::{DefaultFields, Writer};
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::fmt::{FormatFields, FormattedFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::error::{LOG_ALREADY_INITIALIZED, LOG_CONFIG_INVALID};
use crate::rolling::RollingGzipWriter;
use crate::settings::{LogColor, LogFormat, LogSettings};

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// What the subscriber needs to know about the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Environment {
    /// Whether stdout is a terminal: `auto` format and color follow it.
    pub stdout_is_terminal: bool,
    /// Whether `NO_COLOR` is set to a non-empty value.
    pub no_color: bool,
    /// Whether `TERM` is `dumb`.
    pub dumb_terminal: bool,
    /// The service name; the log file is `<log.file.dir>/<service_name>.log`.
    pub service_name: &'static str,
}

/// Keeps the log writer threads running. Dropping it flushes what they still hold.
#[must_use = "dropping the guard stops the log writer threads"]
#[derive(Debug)]
pub struct Guard {
    _stdout: WorkerGuard,
    _file: Option<WorkerGuard>,
}

/// Installs the global subscriber, and the bridge that turns `log` records into events.
///
/// # Errors
///
/// [`LOG_ALREADY_INITIALIZED`] when a global subscriber is already set, checked before the
/// log file is touched; [`LOG_OUTPUT_UNAVAILABLE`](crate::error::LOG_OUTPUT_UNAVAILABLE) when
/// the log file cannot be opened; [`LOG_CONFIG_INVALID`] when the `console` feature was built
/// without `--cfg tokio_unstable`.
pub fn init(settings: &LogSettings, env: &Environment) -> Result<Guard> {
    if tracing::dispatcher::has_been_set() {
        return Error::e_explain(
            LOG_ALREADY_INITIALIZED,
            "a global subscriber is already set",
        );
    }
    if cfg!(feature = "console") && !cfg!(tokio_unstable) {
        let detail = r#"the console feature needs RUSTFLAGS="--cfg tokio_unstable""#;
        return Error::e_explain(LOG_CONFIG_INVALID, detail);
    }
    // Lossless: a full queue makes the logging thread wait rather than drop lines.
    let writer = || NonBlockingBuilder::default().lossy(false);
    let (stdout, stdout_guard) = writer().finish(std::io::stdout());
    let (file, file_guard) = if settings.file.enabled {
        let file = &settings.file;
        let rolling = RollingGzipWriter::open(&file.dir, env.service_name, file.max_archives)?;
        let (file, guard) = writer().finish(rolling);
        (Some(BoxMakeWriter::new(file)), Some(guard))
    } else {
        (None, None)
    };
    let mut layers = layers(settings, env, BoxMakeWriter::new(stdout), file);
    layers.extend(console());
    let installed = Registry::default().with(layers).try_init();
    installed.or_err(
        LOG_ALREADY_INITIALIZED,
        "a global subscriber is already set",
    )?;
    Ok(Guard {
        _stdout: stdout_guard,
        _file: file_guard,
    })
}

/// The subscriber [`init`] installs, without the console layer, writing to the given writers
/// instead of stdout and the log file.
#[must_use]
pub fn build(
    settings: &LogSettings,
    env: &Environment,
    stdout: BoxMakeWriter,
    file: Option<BoxMakeWriter>,
) -> impl Subscriber + Send + Sync {
    Registry::default().with(layers(settings, env, stdout, file))
}

fn layers(
    settings: &LogSettings,
    env: &Environment,
    stdout: BoxMakeWriter,
    file: Option<BoxMakeWriter>,
) -> Vec<BoxedLayer> {
    let json = match settings.format {
        LogFormat::Auto => !env.stdout_is_terminal,
        LogFormat::Text => false,
        LogFormat::Json => true,
    };
    let color = match settings.color {
        LogColor::Always => true,
        LogColor::Never => false,
        LogColor::Auto => env.stdout_is_terminal && !env.no_color && !env.dumb_terminal,
    };
    let stdout_layer = if json {
        let layer = tracing_subscriber::fmt::layer().json();
        layer
            .with_current_span(true)
            .with_span_list(true)
            .with_writer(stdout)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_ansi(color)
            .with_writer(stdout)
            .boxed()
    };
    let mut layers = vec![stdout_layer.with_filter(filter(&settings.filter)).boxed()];
    if let Some(writer) = file {
        let text = if settings.file.filter.is_empty() {
            &settings.filter
        } else {
            &settings.file.filter
        };
        let layer = tracing_subscriber::fmt::layer()
            .fmt_fields(FileFields::default())
            .with_ansi(false)
            .with_writer(writer);
        layers.push(layer.with_filter(filter(text)).boxed());
    }
    layers
}

/// A filter the settings have already checked while loading.
fn filter(text: &str) -> EnvFilter {
    EnvFilter::builder().parse_lossy(text)
}

/// The field formatter of the file layer: the default one under a type of its own. Formatted
/// span fields are kept per formatter type, so the file never reuses fields the stdout layer
/// formatted, with or without color, and never records a field twice.
#[derive(Debug, Default)]
struct FileFields(DefaultFields);

impl<'writer> FormatFields<'writer> for FileFields {
    fn format_fields<R: RecordFields>(&self, writer: Writer<'writer>, fields: R) -> fmt::Result {
        self.0.format_fields(writer, fields)
    }

    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &Record<'_>,
    ) -> fmt::Result {
        if !current.fields.is_empty() {
            current.fields.push(' ');
        }
        self.format_fields(current.as_writer(), fields)
    }
}

/// The tokio-console layer, which keeps the filter console-subscriber gives it.
#[cfg(feature = "console")]
fn console() -> Vec<BoxedLayer> {
    vec![console_subscriber::ConsoleLayer::builder().spawn().boxed()]
}

/// Without the `console` feature there is no console layer.
#[cfg(not(feature = "console"))]
fn console() -> Vec<BoxedLayer> {
    Vec::new()
}
