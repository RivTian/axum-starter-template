//! The in-process event bus: a thin layer over a tokio broadcast channel. Publishing never
//! waits; a subscriber that falls behind skips the oldest events and logs how many.

use tokio::sync::broadcast::{self, error::RecvError};

/// A bus for events of type `E`. Clones share the bus.
#[derive(Debug)]
pub struct EventBus<E: Clone + Send + 'static> {
    sender: broadcast::Sender<E>,
}

impl<E: Clone + Send + 'static> EventBus<E> {
    /// A bus that holds `capacity` events for a slow subscriber; loading the configuration
    /// has checked that it is between 1 and 65536.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        EventBus { sender }
    }

    /// Sends an event to every current subscriber and returns how many there are; with no
    /// subscriber the event is dropped and the result is 0.
    pub fn publish(&self, event: E) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// A stream of the events published from now on.
    #[must_use]
    pub fn subscribe(&self) -> EventStream<E> {
        EventStream {
            receiver: self.sender.subscribe(),
        }
    }
}

impl<E: Clone + Send + 'static> Clone for EventBus<E> {
    fn clone(&self) -> Self {
        EventBus {
            sender: self.sender.clone(),
        }
    }
}

/// One subscriber's view of a bus.
#[derive(Debug)]
pub struct EventStream<E: Clone + Send + 'static> {
    receiver: broadcast::Receiver<E>,
}

impl<E: Clone + Send + 'static> EventStream<E> {
    /// The next event, or `None` once every bus handle is gone. When the subscriber has
    /// fallen behind, the number of skipped events is logged as a warning, and the stream
    /// goes on with the oldest event still held.
    pub async fn recv(&mut self) -> Option<E> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "event subscriber lagged");
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}
