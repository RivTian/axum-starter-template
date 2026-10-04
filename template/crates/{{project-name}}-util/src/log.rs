//! Logging at a level chosen at run time, and logging an error once with its fields. Both
//! macros need only the `tracing` facade, which the calling crate depends on.

/// Emits a `tracing` event at a level chosen at run time, for example the level the HTTP
/// layer picks for a response.
///
/// ```
/// let level = tracing::Level::WARN;
/// svc_util::log_at_level!(level, http.response.status_code = 503, "request failed");
/// ```
#[macro_export]
macro_rules! log_at_level {
    ($level:expr, $($arg:tt)+) => {
        match $level {
            ::tracing::Level::ERROR => ::tracing::event!(::tracing::Level::ERROR, $($arg)+),
            ::tracing::Level::WARN => ::tracing::event!(::tracing::Level::WARN, $($arg)+),
            ::tracing::Level::INFO => ::tracing::event!(::tracing::Level::INFO, $($arg)+),
            ::tracing::Level::DEBUG => ::tracing::event!(::tracing::Level::DEBUG, $($arg)+),
            ::tracing::Level::TRACE => ::tracing::event!(::tracing::Level::TRACE, $($arg)+),
        }
    };
}

/// Logs an error where it is handled, with `error.type`, `error.class`, `error.source`,
/// `error.retry`, `error.context` and `error.chain`, then the given fields and message.
///
/// ```
/// use svc_util::error::{Error, ErrorType};
///
/// let error = Error::explain(ErrorType::InternalError, "store is gone");
/// svc_util::log_error!(tracing::Level::ERROR, &error, service.name = "http", "service failed");
/// ```
#[macro_export]
macro_rules! log_error {
    ($level:expr, $error:expr, $($arg:tt)+) => {{
        let fields = $crate::error::Error::fields($error);
        $crate::log_at_level!(
            $level,
            error.type = fields.etype,
            error.class = fields.class,
            error.source = fields.source,
            error.retry = fields.retry,
            error.context = fields.context.as_str(),
            error.chain = fields.chain.as_str(),
            $($arg)+
        )
    }};
}
