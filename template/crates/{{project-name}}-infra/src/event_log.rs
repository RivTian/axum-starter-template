//! The background service that logs every todo event.

use async_trait::async_trait;
use svc_domain::prelude::*;
use svc_runtime::prelude::*;
use svc_util::prelude::*;

/// The background service `event-log`: logs every todo event at info, with `event.name` and
/// `todo.id`.
#[derive(Debug)]
pub struct EventLog {
    events: EventStream<TodoEvent>,
}

impl EventLog {
    /// A service logging the events of this stream.
    #[must_use]
    pub fn new(events: EventStream<TodoEvent>) -> Self {
        EventLog { events }
    }
}

#[async_trait]
impl Service for EventLog {
    fn name(&self) -> &'static str {
        "event-log"
    }

    async fn run(mut self: Box<Self>, ctx: ServiceContext) -> Result<()> {
        ctx.ready();
        let shutdown = ctx.shutdown();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => return Ok(()),
                event = self.events.recv() => match event {
                    Some(event) => tracing::info!(
                        event.name = event.name(),
                        todo.id = %event.id(),
                        "todo event"
                    ),
                    // Every bus handle is gone: nothing more can arrive.
                    None => return Ok(()),
                },
            }
        }
    }
}
