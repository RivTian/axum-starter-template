//! The error kinds of logging.

use svc_util::prelude::*;

/// The log settings cannot work, such as the `console` feature without `tokio_unstable`.
pub const LOG_CONFIG_INVALID: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "LogConfigInvalid",
    Class::Internal,
    "Invalid log configuration",
));

/// A log output cannot be opened, such as a log directory that cannot be created.
pub const LOG_OUTPUT_UNAVAILABLE: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "LogOutputUnavailable",
    Class::Internal,
    "Log output unavailable",
));

/// A global subscriber is already set.
pub const LOG_ALREADY_INITIALIZED: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "LogAlreadyInitialized",
    Class::Internal,
    "Logging already initialized",
));
