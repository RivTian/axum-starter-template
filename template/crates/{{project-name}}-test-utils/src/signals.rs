//! Signals a test sends by hand.

use svc_runtime::signal::{Signal, SignalSource};
use tokio::sync::mpsc;

/// A [`SignalSource`] fed by a [`SignalSender`].
#[derive(Debug)]
pub struct FakeSignals {
    receiver: mpsc::UnboundedReceiver<Signal>,
}

/// Sends signals to a [`FakeSignals`].
#[derive(Clone, Debug)]
pub struct SignalSender {
    sender: mpsc::UnboundedSender<Signal>,
}

impl FakeSignals {
    /// A source and the sender that feeds it.
    #[must_use]
    pub fn new() -> (Self, SignalSender) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (FakeSignals { receiver }, SignalSender { sender })
    }
}

impl SignalSender {
    /// Delivers a signal; nothing happens once the source is gone.
    pub fn send(&self, signal: Signal) {
        self.sender.send(signal).ok();
    }
}

impl SignalSource for FakeSignals {
    async fn next(&mut self) -> Signal {
        match self.receiver.recv().await {
            Some(signal) => signal,
            // Every sender is gone: no signal will ever come.
            None => std::future::pending().await,
        }
    }
}
