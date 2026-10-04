//! What every handler and middleware can reach.

use std::sync::Arc;

use svc_domain::todo::TodoUseCases;
use svc_runtime::prelude::*;

/// The state shared by the router: the use cases, readiness, and the service name that
/// problem types carry.
#[derive(Clone)]
pub struct AppState {
    pub(crate) todos: Arc<TodoUseCases>,
    pub(crate) readiness: Readiness,
    pub(crate) service_name: &'static str,
}

impl AppState {
    /// The state of a service called `service_name`.
    #[must_use]
    pub fn new(todos: Arc<TodoUseCases>, readiness: Readiness, service_name: &'static str) -> Self {
        AppState {
            todos,
            readiness,
            service_name,
        }
    }
}
