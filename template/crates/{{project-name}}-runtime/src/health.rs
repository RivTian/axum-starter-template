//! Readiness: whether the process should receive traffic, from the lifecycle phase and the
//! health checks behind `/readyz`. A check reports a state it already knows, such as a
//! flag set when a store breaks; it does no I/O, because probes call it on every request.

use std::sync::{Arc, Mutex, PoisonError};

use crate::phase::{Phase, PhaseWatch};

/// The state of one dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// Working.
    Healthy,
    /// Not working, with a short reason for operators.
    Unhealthy(String),
}

/// Something `/readyz` asks before it lets traffic in.
pub trait HealthCheck: Send + Sync {
    /// The current state; cheap and without I/O.
    fn check(&self) -> Health;
}

/// Every check's state, in registration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthReport {
    /// Each check's name and state.
    pub checks: Vec<(&'static str, Health)>,
}

impl HealthReport {
    /// Whether every check is healthy; true when there are none.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.checks
            .iter()
            .all(|(_, health)| *health == Health::Healthy)
    }
}

type Checks = Vec<(&'static str, Arc<dyn HealthCheck>)>;

/// The registered health checks. Clones share the registry.
#[derive(Clone, Default)]
pub struct HealthRegistry {
    checks: Arc<Mutex<Checks>>,
}

impl HealthRegistry {
    /// Adds a check under a name.
    pub fn register(&self, name: &'static str, check: Arc<dyn HealthCheck>) {
        // Pushing to a list cannot leave it half changed, so a poisoned lock still holds a
        // valid list: recover it rather than lose the check.
        self.checks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name, check));
    }

    /// Asks every check for its state.
    #[must_use]
    pub fn report(&self) -> HealthReport {
        // Cloning the list reads it only, so a poisoned lock still gives a valid list.
        let checks = self
            .checks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        HealthReport {
            checks: checks
                .into_iter()
                .map(|(name, check)| (name, check.check()))
                .collect(),
        }
    }
}

impl std::fmt::Debug for HealthRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&'static str> = self
            .checks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(name, _)| *name)
            .collect();
        f.debug_struct("HealthRegistry")
            .field("checks", &names)
            .finish()
    }
}

/// Whether the process should receive traffic, as `/readyz` reports it: only in
/// [`Phase::Running`] and only while every health check passes.
#[derive(Clone, Debug)]
pub struct Readiness {
    phases: PhaseWatch,
    health: HealthRegistry,
}

impl Readiness {
    /// Readiness from the supervisor's phases and the registered health checks.
    #[must_use]
    pub fn new(phases: PhaseWatch, health: HealthRegistry) -> Self {
        Readiness { phases, health }
    }

    /// The current lifecycle phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phases.current()
    }

    /// Every health check's state.
    #[must_use]
    pub fn health(&self) -> HealthReport {
        self.health.report()
    }

    /// Whether traffic should come in: running, and every check healthy.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.phase() == Phase::Running && self.health().is_healthy()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::{Health, HealthCheck, HealthRegistry};

    struct Fixed(Health);

    impl HealthCheck for Fixed {
        fn check(&self) -> Health {
            self.0.clone()
        }
    }

    #[test]
    fn the_report_holds_every_check_in_order() {
        let registry = HealthRegistry::default();
        assert!(registry.report().is_healthy());
        registry.register("store", Arc::new(Fixed(Health::Healthy)));
        registry.register(
            "queue",
            Arc::new(Fixed(Health::Unhealthy("full".to_string()))),
        );
        let report = registry.report();
        assert_eq!(
            report.checks,
            [
                ("store", Health::Healthy),
                ("queue", Health::Unhealthy("full".to_string()))
            ]
        );
        assert!(!report.is_healthy());
    }

    #[test]
    fn a_poisoned_registry_still_registers_and_reports() {
        let registry = HealthRegistry::default();
        let shared = registry.clone();
        let poisoner = thread::spawn(move || {
            let _held = shared.checks.lock();
            std::panic::resume_unwind(Box::new("poison the lock"));
        });
        assert!(poisoner.join().is_err());
        assert!(registry.checks.is_poisoned());
        registry.register("store", Arc::new(Fixed(Health::Healthy)));
        assert_eq!(registry.report().checks, [("store", Health::Healthy)]);
    }
}
