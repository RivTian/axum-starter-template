//! The error kinds of logging.

use svc_util::prelude::*;

/// A log filter does not parse.
pub const LOG_CONFIG_INVALID: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "LogConfigInvalid",
    Class::Internal,
    "Invalid log configuration",
));

/// A global subscriber is already set.
pub const LOG_ALREADY_INITIALIZED: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "LogAlreadyInitialized",
    Class::Internal,
    "Logging already initialized",
));
