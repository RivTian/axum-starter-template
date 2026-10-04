//! The in-process event bus: a thin layer over a tokio broadcast channel. Publishing never
//! waits; a subscriber that falls behind skips the oldest events, is told how many, and
//! logs it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast::{self, error::RecvError};

/// A bus for events of type `E`. Clones share the bus.
#[derive(Debug)]
pub struct EventBus<E: Clone + Send + 'static> {
    sender: broadcast::Sender<E>,
    lagged: Arc<AtomicU64>,
}

impl<E: Clone + Send + 'static> EventBus<E> {
    /// A bus that holds `capacity` events for a slow subscriber; loading the configuration
    /// has checked that it is between 1 and 65536.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        EventBus {
            sender,
            lagged: Arc::new(AtomicU64::new(0)),
        }
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
            lagged: self.lagged.clone(),
        }
    }

    /// How many events subscribers have skipped because they fell behind, in total. Events
    /// dropped for lack of a subscriber are not counted.
    #[must_use]
    pub fn lagged_total(&self) -> u64 {
        self.lagged.load(Ordering::Relaxed)
    }
}

impl<E: Clone + Send + 'static> Clone for EventBus<E> {
    fn clone(&self) -> Self {
        EventBus {
            sender: self.sender.clone(),
            lagged: self.lagged.clone(),
        }
    }
}

/// One subscriber's view of a bus.
#[derive(Debug)]
pub struct EventStream<E: Clone + Send + 'static> {
    receiver: broadcast::Receiver<E>,
    lagged: Arc<AtomicU64>,
}

impl<E: Clone + Send + 'static> EventStream<E> {
    /// The next event, or `None` once every bus handle is gone. When the subscriber has
    /// fallen behind, the skipped events are counted and logged as a warning, and the
    /// stream goes on with the oldest event still held.
    pub async fn recv(&mut self) -> Option<E> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(skipped)) => {
                    self.lagged.fetch_add(skipped, Ordering::Relaxed);
                    tracing::warn!(skipped, "event subscriber lagged");
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}
