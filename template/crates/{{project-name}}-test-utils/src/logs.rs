//! Log events captured while a test runs.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value};
use tracing::subscriber::DefaultGuard;

/// The log events written on this thread while the guard from [`CapturedLogs::start`]
/// lives. Use it with a current-thread runtime, the default of `#[tokio::test]`, so that
/// spawned tasks log on the same thread.
#[derive(Clone, Debug, Default)]
pub struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    /// Starts capturing every event at every level, as JSON lines.
    #[must_use]
    pub fn start() -> (Self, DefaultGuard) {
        let logs = CapturedLogs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    /// Every event so far: its fields, with `message`, plus `level` and `target`.
    #[must_use]
    pub fn events(&self) -> Vec<Map<String, Value>> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|line| {
                let mut fields = line.get("fields")?.as_object()?.clone();
                for key in ["level", "target"] {
                    fields.insert(key.to_string(), line.get(key)?.clone());
                }
                Some(fields)
            })
            .collect()
    }

    /// The events with this message.
    #[must_use]
    pub fn with_message(&self, message: &str) -> Vec<Map<String, Value>> {
        let matches =
            |event: &Map<String, Value>| event.get("message") == Some(&Value::from(message));
        self.events().into_iter().filter(matches).collect()
    }
}

impl Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Appending cannot leave the buffer half changed: recover from a poisoned lock.
        let mut bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
