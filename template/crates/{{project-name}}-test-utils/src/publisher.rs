//! A publisher that records the todo events it receives.

use std::sync::{Mutex, PoisonError};

use svc_domain::prelude::*;

/// Records every published event, in order.
#[derive(Debug, Default)]
pub struct RecordingPublisher {
    events: Mutex<Vec<TodoEvent>>,
}

impl RecordingPublisher {
    /// The events published so far.
    #[must_use]
    pub fn events(&self) -> Vec<TodoEvent> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl TodoPublisher for RecordingPublisher {
    fn publish(&self, event: TodoEvent) {
        // Pushing cannot leave the list half changed: recover from a poisoned lock.
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}
