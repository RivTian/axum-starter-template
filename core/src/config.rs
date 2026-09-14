//! Per-instance hot configuration capabilities. No file I/O or reload policy here.

use crate::TickerInterval;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotConfig {
    pub ticker: TickerInterval,
}

/// Generation and values share one allocation/publication, never separate atomics.
#[derive(Debug)]
pub struct ConfigSnapshot {
    pub generation: u64,
    pub config: HotConfig,
}

/// Non-Clone writer. The app coordinator alone decides whether to publish.
pub struct ConfigPublisher {
    tx: watch::Sender<Arc<ConfigSnapshot>>,
}

#[derive(Clone)]
pub struct ConfigHandle {
    rx: watch::Receiver<Arc<ConfigSnapshot>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    Published(u64),
    Unchanged(u64),
}

impl ConfigPublisher {
    pub fn new(config: HotConfig) -> Self {
        let (tx, _) = watch::channel(Arc::new(ConfigSnapshot {
            generation: 1,
            config,
        }));
        Self { tx }
    }

    pub fn subscribe(&self) -> ConfigHandle {
        ConfigHandle {
            rx: self.tx.subscribe(),
        }
    }
    pub fn current(&self) -> Arc<ConfigSnapshot> {
        self.tx.borrow().clone()
    }

    pub fn publish(&mut self, config: HotConfig) -> Result<Publication, GenerationExhausted> {
        let old = self.current();
        if old.config == config {
            return Ok(Publication::Unchanged(old.generation));
        }
        let generation = old.generation.checked_add(1).ok_or(GenerationExhausted)?;
        // Unlike send(), this retains the new value with zero live readers.
        self.tx
            .send_replace(Arc::new(ConfigSnapshot { generation, config }));
        Ok(Publication::Published(generation))
    }
}

impl ConfigHandle {
    pub fn current(&self) -> Arc<ConfigSnapshot> {
        self.rx.borrow().clone()
    }
    pub async fn changed(&mut self) -> Result<Arc<ConfigSnapshot>, ConfigClosed> {
        self.rx.changed().await.map_err(|_| ConfigClosed)?;
        // Mark exactly the version being cloned; never hold a watch Ref over await.
        Ok(self.rx.borrow_and_update().clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigClosed;
impl std::fmt::Display for ConfigClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("configuration publisher closed")
    }
}
impl std::error::Error for ConfigClosed {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationExhausted;
impl std::fmt::Display for GenerationExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("configuration generation exhausted")
    }
}
impl std::error::Error for GenerationExhausted {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn hot(millis: u64) -> HotConfig {
        HotConfig {
            ticker: TickerInterval::try_from(Duration::from_millis(millis)).unwrap(),
        }
    }

    #[tokio::test]
    async fn no_change_does_not_publish_or_increment_and_reader_gets_latest() {
        let mut publisher = ConfigPublisher::new(hot(1));
        let mut reader = publisher.subscribe();
        assert_eq!(
            publisher.publish(hot(1)).unwrap(),
            Publication::Unchanged(1)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), reader.changed())
                .await
                .is_err()
        );
        assert_eq!(
            publisher.publish(hot(2)).unwrap(),
            Publication::Published(2)
        );
        let snapshot = reader.changed().await.unwrap();
        assert_eq!(snapshot.generation, 2);
        assert_eq!(snapshot.config, hot(2));
    }

    #[test]
    fn publishing_without_readers_is_retained_for_late_subscribers() {
        let mut publisher = ConfigPublisher::new(hot(1));
        publisher.publish(hot(2)).unwrap();
        let late = publisher.subscribe();
        assert_eq!(late.current().generation, 2);
        assert_eq!(late.current().config, hot(2));
    }

    #[tokio::test]
    async fn a_slow_reader_can_skip_generations_but_never_mixes_a_value_and_generation() {
        let mut publisher = ConfigPublisher::new(hot(1));
        let mut reader = publisher.subscribe();
        let producer = async {
            for generation in 2..=100 {
                publisher.publish(hot(generation)).unwrap();
                tokio::task::yield_now().await;
            }
            drop(publisher);
        };
        let consumer = async {
            let mut latest = 1;
            while let Ok(snapshot) = reader.changed().await {
                assert_eq!(
                    snapshot.config.ticker.get().as_millis(),
                    u128::from(snapshot.generation)
                );
                assert!(snapshot.generation > latest);
                latest = snapshot.generation;
            }
            assert_eq!(latest, 100);
        };
        tokio::join!(producer, consumer);
    }

    #[tokio::test]
    async fn a_closed_writer_terminates_waiting_and_overflow_does_not_wrap() {
        let (tx, _) = watch::channel(Arc::new(ConfigSnapshot {
            generation: u64::MAX,
            config: hot(1),
        }));
        let mut publisher = ConfigPublisher { tx };
        let mut reader = publisher.subscribe();
        assert_eq!(publisher.publish(hot(2)), Err(GenerationExhausted));
        assert_eq!(reader.current().generation, u64::MAX);
        assert_eq!(reader.current().config, hot(1));
        drop(publisher);
        assert_eq!(reader.changed().await.unwrap_err(), ConfigClosed);
    }
}
