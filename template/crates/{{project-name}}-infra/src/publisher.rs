//! Todo events onto the runtime's event bus.

use svc_domain::prelude::*;
use svc_runtime::prelude::*;

/// Publishes todo events on an event bus. With no subscriber an event is dropped: delivery is
/// at most once.
#[derive(Clone, Debug)]
pub struct BusTodoPublisher {
    bus: EventBus<TodoEvent>,
}

impl BusTodoPublisher {
    /// A publisher onto this bus.
    #[must_use]
    pub fn new(bus: EventBus<TodoEvent>) -> Self {
        BusTodoPublisher { bus }
    }
}

impl TodoPublisher for BusTodoPublisher {
    fn publish(&self, event: TodoEvent) {
        self.bus.publish(event);
    }
}
