//! The configuration sections the runtime owns.

use std::time::Duration;

/// `[lifecycle]`: start-up and shutdown timing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleSettings {
    /// `lifecycle.startup_timeout`: every service must be ready within it.
    pub startup_timeout: Duration,
    /// `lifecycle.drain_delay`: after SIGTERM, requests are still served this long, so that a
    /// load balancer can take the instance out.
    pub drain_delay: Duration,
    /// `lifecycle.drain_timeout`: once services are asked to stop, they get this long.
    pub drain_timeout: Duration,
}

impl Default for LifecycleSettings {
    fn default() -> Self {
        LifecycleSettings {
            startup_timeout: Duration::from_secs(30),
            drain_delay: Duration::from_secs(5),
            drain_timeout: Duration::from_secs(20),
        }
    }
}

/// `[events]`: the in-process event bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventSettings {
    /// `events.capacity`: how many events the bus holds for a slow subscriber.
    pub capacity: usize,
}

impl Default for EventSettings {
    fn default() -> Self {
        EventSettings { capacity: 1024 }
    }
}
