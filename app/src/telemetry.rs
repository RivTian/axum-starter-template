//! The only production subscriber installation point. Libraries never call it.

use crate::shutdown::{RunReport, StopCause};
use crate::supervisor::{ExitKind, TaskExit};
use std::ffi::OsStr;
use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

pub(crate) fn init() -> Result<(), &'static str> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => EnvFilter::try_new(value).map_err(|_| "invalid RUST_LOG filter")?,
        Err(std::env::VarError::NotPresent) => EnvFilter::new("info"),
        Err(_) => return Err("RUST_LOG must be UTF-8"),
    };
    let no_color = std::env::var_os("NO_COLOR");
    let term = std::env::var_os("TERM");
    // Detect the actual log sink, not stderr (which Cargo may still color).
    // Keep redirected logs plain; color changes presentation, never event fields.
    let ansi = ansi_enabled(
        std::io::stdout().is_terminal(),
        no_color.as_deref(),
        term.as_deref(),
    );
    tracing_subscriber::fmt()
        .compact()
        .with_writer(std::io::stdout)
        .with_ansi(ansi)
        .with_target(false)
        .with_env_filter(filter)
        .try_init()
        .map_err(|_| "tracing subscriber initialization failed")
}

fn ansi_enabled(is_terminal: bool, no_color: Option<&OsStr>, term: Option<&OsStr>) -> bool {
    is_terminal && no_color.is_none_or(OsStr::is_empty) && term != Some(OsStr::new("dumb"))
}

fn task_exit(task: &TaskExit) {
    // Current face errors deliberately redact Display; never log panic payloads,
    // Debug, or recursively dump a driver's source chain here.
    if let ExitKind::Failed(error) = &task.kind {
        tracing::error!(task = task.name, runtime = %task.runtime, kind = task.kind.label(), error = %error, "task exit summary");
    } else {
        tracing::info!(
            task = task.name,
            runtime = %task.runtime,
            kind = task.kind.label(),
            "task exit summary"
        );
    }
}

pub(crate) fn report(report: &RunReport, elapsed: std::time::Duration) {
    match &report.cause {
        StopCause::Task(exit) => task_exit(exit),
        StopCause::Startup { phase, source } => {
            tracing::error!(phase, error = %source, "startup failed")
        }
        StopCause::ReloadFailure(kind) => tracing::error!(kind, "configuration loader failed"),
        StopCause::StartupTimeout(phase) => tracing::error!(phase, "startup deadline exceeded"),
        _ => {}
    }
    for task in &report.tasks {
        task_exit(task);
    }
    let aborted = report
        .tasks
        .iter()
        .filter(|task| matches!(task.kind, ExitKind::Cancelled))
        .count();
    tracing::info!(event = "shutdown_complete", elapsed_ms = elapsed.as_millis(), cause = report.cause.label(), forced = report.forced,
        storage = report.storage.label(), aborted_tasks = aborted, unreaped_tasks = report.unreaped.len(),
        unreaped = ?report.unreaped, cleanup_failed = report.cleanup_failed,
        exit_code = u8::from(!report.succeeded()), "service shutdown completed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_policy_depends_on_the_sink_and_startup_environment() {
        // Pure inputs: do not mutate process environment or install a subscriber.
        for (terminal, no_color, term, expected) in [
            (true, None, Some("xterm-256color"), true),
            (true, None, None, true),
            (true, Some(""), Some("xterm-256color"), true),
            (true, Some("1"), Some("xterm-256color"), false),
            (true, Some("0"), None, false),
            (true, None, Some("dumb"), false),
            (true, Some(""), Some("dumb"), false),
            (false, None, Some("xterm-256color"), false),
            (false, None, None, false),
            (false, Some(""), Some("xterm-256color"), false),
            (false, Some("1"), Some("xterm-256color"), false),
        ] {
            assert_eq!(
                ansi_enabled(terminal, no_color.map(OsStr::new), term.map(OsStr::new)),
                expected,
                "terminal={terminal}, NO_COLOR={no_color:?}, TERM={term:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_no_color_is_still_a_nonempty_opt_out() {
        use std::os::unix::ffi::OsStrExt;

        assert!(!ansi_enabled(true, Some(OsStr::from_bytes(b"\xff")), None));
    }
}
