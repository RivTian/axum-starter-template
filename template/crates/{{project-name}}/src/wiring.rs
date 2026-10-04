//! Assembling the services from the configuration. A new feature adds its adapters and
//! services here.

use std::sync::Arc;

use svc_api::server::HttpServer;
use svc_api::state::AppState;
use svc_domain::todo::{TodoEvent, TodoUseCases};
use svc_infra::clock::SystemClock;
use svc_infra::event_log::EventLog;
use svc_infra::memory::InMemoryTodoRepository;
use svc_infra::publisher::BusTodoPublisher;
use svc_runtime::prelude::*;
use svc_runtime::supervisor::Supervisor;

use crate::names::SERVICE_NAME;
use crate::settings::Config;

/// The supervisor with every service, reporting its phase through `phases`.
pub(crate) fn supervisor(config: &Config, phases: &PhaseWatch) -> Supervisor {
    let bus = EventBus::<TodoEvent>::new(config.events.capacity);
    let todos = TodoUseCases::new(
        Arc::new(InMemoryTodoRepository::default()),
        Arc::new(BusTodoPublisher::new(bus.clone())),
        Arc::new(SystemClock),
    );
    let readiness = Readiness::new(phases.clone(), HealthRegistry::default());
    let state = AppState::new(Arc::new(todos), readiness, SERVICE_NAME);
    Supervisor::new(&config.lifecycle, phases.clone())
        .with(HttpServer::new(&config.server, state))
        .with(EventLog::new(bus.subscribe()))
}
