//! The panic hook: a panic becomes an error event with target `panic` when such events are
//! enabled; otherwise the previous hook (by default, the message on stderr) runs. Installed
//! first thing, it also covers panics before logging is up.

use std::panic::PanicHookInfo;

/// Installs a panic hook that logs a panic as an ERROR event with target `panic` when
/// such events are enabled, and otherwise hands it to the previous hook, so a panic is
/// never silent, not even with the filter `off`.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if tracing::enabled!(target: "panic", tracing::Level::ERROR) {
            let current = std::thread::current();
            let thread_name = current.name().unwrap_or("<unnamed>");
            // `event!` rather than `error!`: tracing's `error!` cannot parse a dotted field
            // name right after `target:`.
            tracing::event!(
                target: "panic",
                tracing::Level::ERROR,
                panic.message = %message(info),
                panic.location = %location(info),
                thread.name = thread_name,
                "panic"
            );
        } else {
            previous(info);
        }
    }));
}

fn message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string payload>".to_string())
}

fn location(info: &PanicHookInfo<'_>) -> String {
    info.location()
        .map_or_else(String::new, ToString::to_string)
}
