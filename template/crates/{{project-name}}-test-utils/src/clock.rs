//! A clock that tests control.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use svc_domain::prelude::*;
use time::OffsetDateTime;
use time::macros::datetime;

/// A clock that starts at a fixed instant and moves one millisecond forward on every reading,
/// so what it times comes out in order.
#[derive(Debug)]
pub struct FakeClock {
    next: Mutex<OffsetDateTime>,
}

impl FakeClock {
    /// A clock whose first reading is `start`.
    #[must_use]
    pub fn new(start: OffsetDateTime) -> Self {
        FakeClock {
            next: Mutex::new(start),
        }
    }
}

impl Default for FakeClock {
    /// A clock starting at 2026-01-01 00:00 UTC.
    fn default() -> Self {
        Self::new(datetime!(2026-01-01 00:00 UTC))
    }
}

impl Clock for FakeClock {
    fn now(&self) -> OffsetDateTime {
        // Reading and advancing one value cannot leave it half changed: recover.
        let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        let now = *next;
        *next = now + Duration::from_millis(1);
        now
    }
}
