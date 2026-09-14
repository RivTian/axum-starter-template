//! Shared values only: no configuration I/O, runtime construction or global state.

pub mod config;
pub mod lifecycle;

use std::fmt;
use std::time::Duration;

/// Constructed by the executable so the service name is not this library's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    pub service: &'static str,
    pub version: &'static str,
}

/// A nonzero ticker period: Tokio rejects zero-length intervals.
/// Application-specific range limits belong in the configuration loader; the
/// timer owner must also check that its next deadline is representable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickerInterval(Duration);

impl TickerInterval {
    pub fn get(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for TickerInterval {
    type Error = InvalidTickerInterval;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        if value.is_zero() {
            Err(InvalidTickerInterval)
        } else {
            Ok(Self(value))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTickerInterval;

impl fmt::Display for InvalidTickerInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ticker interval must be greater than zero")
    }
}

impl std::error::Error for InvalidTickerInterval {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_period_is_rejected_before_constructing_a_timer() {
        assert_eq!(
            TickerInterval::try_from(Duration::ZERO),
            Err(InvalidTickerInterval)
        );
        let period = Duration::from_secs(1);
        assert_eq!(TickerInterval::try_from(period).unwrap().get(), period);
    }
}
