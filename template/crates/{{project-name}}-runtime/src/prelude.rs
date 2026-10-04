//! Items that other crates import with `use svc_runtime::prelude::*;`: what a service
//! implementation and the code that assembles services need.

pub use crate::bus::{EventBus, EventStream};
pub use crate::health::{Health, HealthCheck, HealthRegistry, Readiness};
pub use crate::phase::{Phase, PhaseWatch};
pub use crate::service::{Service, ServiceContext, ServiceKind};
