//! The clock of the operating system.

use svc_domain::prelude::*;
use time::OffsetDateTime;

/// The system clock, in UTC.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}
