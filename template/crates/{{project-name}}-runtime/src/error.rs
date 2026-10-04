//! The error kinds of the runtime.

use svc_util::prelude::*;

/// A service returned when it should have kept running: before it was ready, or, for a
/// frontline service, before it was asked to stop.
pub const SERVICE_EXITED: ErrorType =
    ErrorType::Kind(&ErrorKind::new("ServiceExited", Class::Internal));

/// A service returned an error; the cause is that error.
pub const SERVICE_FAILED: ErrorType =
    ErrorType::Kind(&ErrorKind::new("ServiceFailed", Class::Internal));

/// A service panicked.
pub const SERVICE_PANICKED: ErrorType =
    ErrorType::Kind(&ErrorKind::new("ServicePanicked", Class::Internal));

/// Not every service was ready within `lifecycle.startup_timeout`.
pub const STARTUP_TIMED_OUT: ErrorType =
    ErrorType::Kind(&ErrorKind::new("StartupTimedOut", Class::Timeout));

/// The signal handlers could not be installed.
pub const SIGNAL_SETUP_FAILED: ErrorType =
    ErrorType::Kind(&ErrorKind::new("SignalSetupFailed", Class::Internal));
