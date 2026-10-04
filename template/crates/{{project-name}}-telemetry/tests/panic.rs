//! The panic hook: a panic is logged when events with target `panic` are enabled, and handed
//! to the previous hook, by default the message on stderr, when they are not. Panic hooks are
//! global, so these tests live in a binary of their own.

use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use svc_telemetry::settings::LogSettings;
use svc_telemetry::subscriber::{Environment, build};
use tracing_subscriber::fmt::writer::BoxMakeWriter;

/// How often the hook that was installed before ours ran.
static PREVIOUS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[expect(clippy::panic, reason = "only a real panic runs the panic hook")]
fn panic_with_filter(filter: &str) -> String {
    let output = Output::default();
    let writer = output.clone();
    let settings = LogSettings {
        filter: filter.to_string(),
        ..LogSettings::default()
    };
    let env = Environment {
        stdout_is_terminal: false,
        no_color: false,
        dumb_terminal: false,
        service_name: "svc",
    };
    let subscriber = build(
        &settings,
        &env,
        BoxMakeWriter::new(move || writer.clone()),
        None,
    );
    tracing::subscriber::with_default(subscriber, || {
        std::panic::catch_unwind(|| std::panic::panic_any("scripted panic")).ok();
    });
    let bytes = output
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn a_panic_is_logged_or_handed_to_the_previous_hook() {
    std::panic::set_hook(Box::new(|_| {
        PREVIOUS.fetch_add(1, Ordering::SeqCst);
    }));
    svc_telemetry::panic::install();

    let logged = panic_with_filter("info");
    assert!(
        logged.contains("\"target\":\"panic\"") && logged.contains("scripted panic"),
        "{logged}"
    );
    assert_eq!(PREVIOUS.load(Ordering::SeqCst), 0);

    let silent = panic_with_filter("off");
    assert_eq!(silent, "");
    assert_eq!(
        PREVIOUS.load(Ordering::SeqCst),
        1,
        "with the filter off the previous hook runs"
    );
}
