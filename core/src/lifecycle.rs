//! Per-instance lifecycle capability. This is not configuration hot reload.

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Booting,
    Running,
    Draining,
    Forcing,
}

/// Non-Clone writer; only the composition root receives this capability.
pub struct LifecyclePublisher {
    tx: watch::Sender<Phase>,
}

#[derive(Clone)]
pub struct LifecycleHandle {
    rx: watch::Receiver<Phase>,
}

pub fn channel() -> (LifecyclePublisher, LifecycleHandle) {
    let (tx, rx) = watch::channel(Phase::Booting);
    (LifecyclePublisher { tx }, LifecycleHandle { rx })
}

impl LifecyclePublisher {
    pub fn publish(&mut self, phase: Phase) {
        self.tx.send_replace(phase);
    }
}

impl LifecycleHandle {
    /// A dead writer is never readiness, even if the last snapshot said Running.
    pub fn phase(&self) -> Option<Phase> {
        self.rx.has_changed().ok()?;
        Some(*self.rx.borrow())
    }

    /// Read first, then wait: committing before this future is polled is valid.
    /// No watch borrow escapes across an await.
    pub async fn wait_running(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<bool, LifecycleClosed> {
        loop {
            if cancel.is_cancelled() {
                return Ok(false);
            }
            match self.phase().ok_or(LifecycleClosed)? {
                Phase::Running => return Ok(true),
                Phase::Draining | Phase::Forcing => return Ok(false),
                Phase::Booting => {}
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(false),
                result = self.rx.changed() => {
                    if result.is_err() {
                        return if cancel.is_cancelled() { Ok(false) } else { Err(LifecycleClosed) };
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleClosed;
impl std::fmt::Display for LifecycleClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("lifecycle publisher closed")
    }
}
impl std::error::Error for LifecycleClosed {}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn an_early_commit_is_observed_and_a_closed_writer_is_not_ready() {
        let (mut writer, mut reader) = channel();
        writer.publish(Phase::Running);
        assert!(
            reader
                .wait_running(&CancellationToken::new())
                .await
                .unwrap()
        );
        drop(writer);
        assert_eq!(reader.phase(), None);
        assert_eq!(
            reader.wait_running(&CancellationToken::new()).await,
            Err(LifecycleClosed)
        );
    }
    #[tokio::test]
    async fn cancelling_before_commit_does_not_start_work() {
        let (_writer, mut reader) = channel();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(!reader.wait_running(&cancel).await.unwrap());
    }
}
