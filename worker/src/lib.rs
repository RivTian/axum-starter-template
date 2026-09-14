//! The ticker's caller owns its runtime, startup commit and task lifetime.

use service_core::TickerInterval;
use service_core::config::{ConfigClosed, ConfigHandle, ConfigSnapshot};
use service_core::lifecycle::{LifecycleClosed, LifecycleHandle};
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("ticker startup receiver closed")]
    StartupAbandoned,
    #[error(transparent)]
    Lifecycle(#[from] LifecycleClosed),
    #[error(transparent)]
    Configuration(#[from] ConfigClosed),
    #[error("ticker period cannot be represented by the monotonic clock")]
    InvalidDeadline,
}

pub async fn run(
    mut config: ConfigHandle,
    mut lifecycle: LifecycleHandle,
    shutdown: CancellationToken,
    started: oneshot::Sender<()>,
) -> Result<(), WorkerError> {
    let mut active = config.current();
    let prepared_timer = make_timer(active.config.ticker)?;
    if started.send(()).is_err() {
        return if shutdown.is_cancelled() {
            Ok(())
        } else {
            Err(WorkerError::StartupAbandoned)
        };
    }
    if !lifecycle.wait_running(&shutdown).await? {
        return Ok(());
    }
    // Waiting behind the startup gate must not consume the first work period.
    drop(prepared_timer);
    active = config.current();
    let mut timer = make_timer(active.config.ticker)?;
    applied(&active);
    let mut sequence = 0_u64;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return Ok(()),
            changed = config.changed() => match changed {
                Ok(snapshot) => {
                    if apply_snapshot(&mut timer, &mut active, snapshot)? { applied(&active); }
                }
                Err(error) => return if shutdown.is_cancelled() { Ok(()) } else { Err(error.into()) },
            },
            _ = timer.tick() => {
                sequence = sequence.saturating_add(1);
                tracing::info!(tick_seq = sequence, config_generation = active.generation,
                    interval_ms = active.config.ticker.get().as_millis(), "ticker tick");
            }
        }
    }
}

fn applied(snapshot: &ConfigSnapshot) {
    tracing::info!(
        event = "ticker_config_applied",
        config_generation = snapshot.generation,
        interval_ms = snapshot.config.ticker.get().as_millis(),
        "ticker observed configuration"
    );
}

fn apply_snapshot(
    timer: &mut Interval,
    active: &mut Arc<ConfigSnapshot>,
    next: Arc<ConfigSnapshot>,
) -> Result<bool, WorkerError> {
    // current() at the startup gate may already have read this watch version.
    // Marking it seen later must not reset the timer or log consumption twice.
    if next.generation <= active.generation {
        return Ok(false);
    }
    // reset_at only changes the next deadline, NOT the stored period.
    // Recreate the interval so every following tick uses the new period.
    *timer = make_timer(next.config.ticker)?;
    *active = next;
    Ok(true)
}

fn next_deadline(period: TickerInterval) -> Result<Instant, WorkerError> {
    Instant::now()
        .checked_add(period.get())
        .ok_or(WorkerError::InvalidDeadline)
}

fn make_timer(period: TickerInterval) -> Result<Interval, WorkerError> {
    let mut timer = interval_at(next_deadline(period)?, period.get());
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    Ok(timer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use service_core::config::{ConfigPublisher, HotConfig};
    use service_core::lifecycle::{self, Phase};
    use std::time::Duration;
    use tokio::time::{advance, timeout};

    #[tokio::test(start_paused = true)]
    async fn first_tick_waits_a_full_period_and_missed_ticks_are_skipped() {
        let period = Duration::from_secs(1);
        let start = Instant::now();
        let mut timer = make_timer(TickerInterval::try_from(period).unwrap()).unwrap();
        assert!(
            timeout(Duration::from_millis(999), timer.tick())
                .await
                .is_err()
        );
        assert_eq!(timer.tick().await, start + period);
        advance(Duration::from_secs(10)).await;
        timer.tick().await;
        assert!(
            timeout(Duration::from_millis(100), timer.tick())
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn startup_gate_delays_work_and_cancellation_closes_the_task() {
        let (mut writer, reader) = lifecycle::channel();
        let cancel = CancellationToken::new();
        let (tx, rx) = oneshot::channel();
        let child = cancel.child_token();
        let mut tasks = tokio::task::JoinSet::new();
        let publisher = ConfigPublisher::new(HotConfig {
            ticker: TickerInterval::try_from(Duration::from_secs(1)).unwrap(),
        });
        tasks.spawn(run(publisher.subscribe(), reader, child, tx));
        rx.await.unwrap();
        advance(Duration::from_secs(10)).await;
        assert!(tasks.try_join_next().is_none());
        writer.publish(Phase::Running);
        cancel.cancel();
        tasks.join_next().await.unwrap().unwrap().unwrap();
    }

    #[tokio::test]
    async fn abandoned_ack_is_an_error_unless_shutdown_already_started() {
        let (_writer, reader) = lifecycle::channel();
        let (tx, rx) = oneshot::channel();
        drop(rx);
        let publisher = ConfigPublisher::new(HotConfig {
            ticker: TickerInterval::try_from(Duration::from_secs(1)).unwrap(),
        });
        assert!(matches!(
            run(
                publisher.subscribe(),
                reader.clone(),
                CancellationToken::new(),
                tx
            )
            .await,
            Err(WorkerError::StartupAbandoned)
        ));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, rx) = oneshot::channel();
        drop(rx);
        run(publisher.subscribe(), reader, cancel, tx)
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    use service_core::{
        config::{ConfigPublisher, HotConfig},
        lifecycle::{self, Phase},
    };
    use std::time::Duration;
    use tokio::time::{advance, timeout};

    fn hot(millis: u64) -> HotConfig {
        HotConfig {
            ticker: TickerInterval::try_from(Duration::from_millis(millis)).unwrap(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn changing_period_discards_the_old_deadline_without_an_extra_tick() {
        let mut publisher = ConfigPublisher::new(hot(1000));
        let mut active = publisher.current();
        let mut timer = make_timer(active.config.ticker).unwrap();
        advance(Duration::from_millis(900)).await;
        publisher.publish(hot(2000)).unwrap();
        let observed = Instant::now();
        assert!(apply_snapshot(&mut timer, &mut active, publisher.current()).unwrap());
        assert!(
            timeout(Duration::from_millis(1999), timer.tick())
                .await
                .is_err()
        );
        assert_eq!(timer.tick().await, observed + Duration::from_millis(2000));
        advance(Duration::from_millis(500)).await;
        assert!(!apply_snapshot(&mut timer, &mut active, publisher.current()).unwrap());
        // Receiving an already-read generation must not restart its period.
        assert_eq!(timer.tick().await, observed + Duration::from_millis(4000));
    }

    #[tokio::test]
    async fn writer_closure_is_failure_unless_root_cancellation_already_won() {
        let (mut lifecycle, reader) = lifecycle::channel();
        lifecycle.publish(Phase::Running);
        let publisher = ConfigPublisher::new(hot(1000));
        let config = publisher.subscribe();
        drop(publisher);
        let (tx, _rx) = oneshot::channel();
        let error = run(config.clone(), reader.clone(), CancellationToken::new(), tx)
            .await
            .unwrap_err();
        assert!(matches!(error, WorkerError::Configuration(ConfigClosed)));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, _rx) = oneshot::channel();
        run(config, reader, cancel, tx).await.unwrap();
    }
}
