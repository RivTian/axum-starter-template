//! The current time, as the domain sees it.

use time::OffsetDateTime;

/// The current time. Infra reads the system clock; tests use a clock they control.
pub trait Clock: Send + Sync {
    /// Now, in UTC.
    fn now(&self) -> OffsetDateTime;
}
