//! Persistent OS subscriptions; no detached forwarding task and no resubscribe gap.

#[derive(Clone, Copy, Debug)]
pub(crate) enum Event {
    Stop,
    Reload,
}
#[derive(Debug, thiserror::Error)]
#[error("control event source closed")]
pub(crate) struct ControlClosed;
pub(crate) type ControlResult = Result<Event, ControlClosed>;

#[cfg(unix)]
pub(crate) struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    reload: tokio::signal::unix::Signal,
}
#[cfg(unix)]
impl Signals {
    pub(crate) fn install() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            reload: signal(SignalKind::hangup())?,
        })
    }
    pub(crate) async fn next(&mut self) -> ControlResult {
        tokio::select! {
            biased;
            value = self.interrupt.recv() => value.map(|_| Event::Stop).ok_or(ControlClosed),
            value = self.terminate.recv() => value.map(|_| Event::Stop).ok_or(ControlClosed),
            value = self.reload.recv() => value.map(|_| Event::Reload).ok_or(ControlClosed),
        }
    }
}

#[cfg(windows)]
pub(crate) struct Signals {
    interrupt: tokio::signal::windows::CtrlC,
}
#[cfg(windows)]
impl Signals {
    pub(crate) fn install() -> std::io::Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::windows::ctrl_c()?,
        })
    }
    pub(crate) async fn next(&mut self) -> ControlResult {
        self.interrupt
            .recv()
            .await
            .map(|_| Event::Stop)
            .ok_or(ControlClosed)
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("this template requires an explicit signal adapter for the target OS");
