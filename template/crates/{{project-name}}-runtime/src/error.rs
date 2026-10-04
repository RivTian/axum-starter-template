//! The error kinds of the runtime.

use svc_util::prelude::*;

/// A service returned when it should have kept running: before it was ready, or, for a
/// frontline service, before it was asked to stop.
pub const SERVICE_EXITED: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "ServiceExited",
    Class::Internal,
    "Service exited",
));

/// A service returned an error; the cause is that error.
pub const SERVICE_FAILED: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "ServiceFailed",
    Class::Internal,
    "Service failed",
));

/// A service panicked.
pub const SERVICE_PANICKED: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "ServicePanicked",
    Class::Internal,
    "Service panicked",
));

/// Not every service was ready within `lifecycle.startup_timeout`.
pub const STARTUP_TIMED_OUT: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "StartupTimedOut",
    Class::Timeout,
    "Startup timed out",
));

/// The signal handlers could not be installed.
pub const SIGNAL_SETUP_FAILED: ErrorType = ErrorType::Kind(&ErrorKind::new(
    "SignalSetupFailed",
    Class::Internal,
    "Signal setup failed",
));
