//! The configuration sections the runtime owns.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// `[lifecycle]`: start-up and shutdown timing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleSettings {
    /// `lifecycle.startup_timeout`: every service must be ready within it; 1s to 600s.
    #[serde(
        serialize_with = "svc_util::duration::serialize",
        deserialize_with = "svc_util::de::duration::<_, 1, 600>"
    )]
    pub startup_timeout: Duration,
    /// `lifecycle.drain_delay`: after SIGTERM, requests are still served this long, so that a
    /// load balancer can take the instance out; 0s to 60s.
    #[serde(
        serialize_with = "svc_util::duration::serialize",
        deserialize_with = "svc_util::de::duration::<_, 0, 60>"
    )]
    pub drain_delay: Duration,
    /// `lifecycle.drain_timeout`: once services are asked to stop, they get this long; 1s to
    /// 300s.
    #[serde(
        serialize_with = "svc_util::duration::serialize",
        deserialize_with = "svc_util::de::duration::<_, 1, 300>"
    )]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSettings {
    /// `events.capacity`: how many events the bus holds for a slow subscriber; 1 to 65536.
    #[serde(deserialize_with = "svc_util::de::integer::<_, 1, 65_536>")]
    pub capacity: usize,
}

impl Default for EventSettings {
    fn default() -> Self {
        EventSettings { capacity: 1024 }
    }
}
