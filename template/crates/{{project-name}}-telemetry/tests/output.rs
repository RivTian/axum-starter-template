//! What the outputs write: JSON, text and pretty lines, a filter per output, a log file that
//! never repeats a field nor carries color codes, and a second `init` that touches nothing.

use std::error::Error;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use svc_telemetry::settings::{LogColor, LogFormat, LogSettings};
use svc_telemetry::subscriber::{Environment, build, init};
use tracing_subscriber::fmt::writer::BoxMakeWriter;

type TestResult = Result<(), Box<dyn Error>>;

/// What one output received.
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Output {
    fn writer(&self) -> BoxMakeWriter {
        let output = self.clone();
        BoxMakeWriter::new(move || output.clone())
    }

    fn text(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const ENV: Environment = Environment {
    stdout_is_terminal: false,
    no_color: false,
    dumb_terminal: false,
    service_name: "svc",
};

#[test]
fn json_lines_carry_level_target_fields_and_every_span_once() -> TestResult {
    let stdout = Output::default();
    let settings = LogSettings::default();
    let subscriber = build(&settings, &ENV, stdout.writer(), None);
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::error_span!("request", request_id = "r-1");
        let _entered = span.enter();
        let inner = tracing::error_span!("handler");
        let _inner = inner.enter();
        tracing::info!(http.response.status_code = 200, "request finished");
    });
    let line: Value = serde_json::from_str(stdout.text().trim())?;
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["target"], "output");
    assert_eq!(line["fields"]["message"], "request finished");
    assert_eq!(line["fields"]["http.response.status_code"], 200);
    assert_eq!(line["spans"][0]["request_id"], "r-1");
    assert_eq!(line["spans"][1]["name"], "handler");
    assert!(line.get("span").is_none(), "{line}");
    assert!(line["timestamp"].as_str().is_some_and(|t| t.ends_with('Z')));
    Ok(())
}

#[test]
fn pretty_text_spreads_an_event_over_lines_but_the_file_keeps_one() {
    let (stdout, file) = (Output::default(), Output::default());
    let settings = LogSettings {
        format: LogFormat::Pretty,
        ..LogSettings::default()
    };
    let subscriber = build(&settings, &ENV, stdout.writer(), Some(file.writer()));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::error_span!("request", request_id = "r-1");
        let _entered = span.enter();
        tracing::info!(http.response.status_code = 200, "request finished");
    });
    let stdout = stdout.text();
    assert!(stdout.lines().count() > 1, "{stdout}");
    assert!(stdout.contains("request finished"), "{stdout}");
    assert!(stdout.contains("output.rs:"), "{stdout}");
    assert!(stdout.contains("request_id"), "{stdout}");
    assert!(!stdout.contains('\u{1b}'), "{stdout:?}");
    assert_eq!(file.text().lines().count(), 1, "{}", file.text());
}

#[test]
fn each_output_has_its_own_filter() {
    let (stdout, file) = (Output::default(), Output::default());
    let mut settings = LogSettings {
        filter: "warn".to_string(),
        ..LogSettings::default()
    };
    settings.file.filter = "debug".to_string();
    let subscriber = build(&settings, &ENV, stdout.writer(), Some(file.writer()));
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!("detail");
        tracing::warn!("trouble");
    });
    assert_eq!(stdout.text().lines().count(), 1);
    assert_eq!(file.text().lines().count(), 2);
}

#[test]
fn the_file_repeats_no_field_and_carries_no_color_codes() {
    let (stdout, file) = (Output::default(), Output::default());
    let settings = LogSettings {
        format: LogFormat::Text,
        color: LogColor::Always,
        ..LogSettings::default()
    };
    let subscriber = build(&settings, &ENV, stdout.writer(), Some(file.writer()));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::error_span!("request", status = tracing::field::Empty);
        span.record("status", 200);
        let _entered = span.enter();
        tracing::info!("request finished");
    });
    let file = file.text();
    assert_eq!(file.matches("status=200").count(), 1, "{file}");
    assert!(!file.contains('\u{1b}'), "{file:?}");
    assert!(stdout.text().contains('\u{1b}'));
}

#[test]
fn auto_color_follows_the_terminal_no_color_and_term() {
    let colored = |env: Environment| {
        let stdout = Output::default();
        let settings = LogSettings {
            format: LogFormat::Text,
            ..LogSettings::default()
        };
        tracing::subscriber::with_default(build(&settings, &env, stdout.writer(), None), || {
            tracing::info!("hello");
        });
        stdout.text().contains('\u{1b}')
    };
    let terminal = Environment {
        stdout_is_terminal: true,
        ..ENV
    };
    assert!(colored(terminal));
    assert!(!colored(Environment {
        no_color: true,
        ..terminal
    }));
    assert!(!colored(Environment {
        dumb_terminal: true,
        ..terminal
    }));
    assert!(!colored(ENV));
}

#[test]
fn a_second_init_fails_before_it_touches_the_log_file() -> TestResult {
    let dir = std::env::temp_dir().join(format!("svc-telemetry-init-{}", std::process::id()));
    let mut settings = LogSettings::default();
    settings.file.enabled = true;
    settings.file.dir.clone_from(&dir);
    let guard = init(&settings, &ENV)?;
    tracing::warn!("first");
    let second = init(&settings, &ENV)
        .err()
        .map(|error| error.fields().etype);
    assert_eq!(second, Some("LogAlreadyInitialized"));
    log::warn!("through the log bridge");
    drop(guard);
    let mut names: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    assert_eq!(names, ["svc.log"]);
    let text = std::fs::read_to_string(dir.join("svc.log"))?;
    assert!(
        text.contains("first") && text.contains("through the log bridge"),
        "{text}"
    );
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

#[cfg(all(feature = "console", not(tokio_unstable)))]
#[test]
fn the_console_needs_tokio_unstable() {
    let error = init(&LogSettings::default(), &ENV)
        .err()
        .map(|e| e.fields());
    let error = error.map(|fields| (fields.etype, fields.context));
    let expected = r#"the console feature needs RUSTFLAGS="--cfg tokio_unstable""#;
    assert_eq!(error, Some(("LogConfigInvalid", expected.to_string())));
}
