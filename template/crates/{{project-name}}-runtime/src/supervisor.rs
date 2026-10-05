//! The supervisor: starts every service, feeds everything that happens into one ordered
//! channel, and carries out what the state machine in `machine` decides. Readiness, exits,
//! panics, signals and timers all arrive through that channel and are handled one at a time,
//! so the same sequence of inputs always ends the same way.

mod machine;

use std::any::Any;

use svc_util::prelude::*;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinError;

use crate::error::{SERVICE_EXITED, STARTUP_TIMED_OUT};
use crate::phase::{Phase, PhaseWatch};
use crate::service::{Service, ServiceContext, ServiceKind};
use crate::settings::LifecycleSettings;
use crate::signal::{Signal, SignalSource};
use machine::{Effect, State};

/// How a run ended. The binary maps each outcome to an exit code.
#[derive(Debug)]
pub enum Outcome {
    /// Every service stopped in time after a signal.
    Stopped,
    /// A service returned, failed or panicked before every service was ready, or startup
    /// timed out.
    StartupFailed(BError),
    /// A service failed, panicked or returned early while running, or failed while stopping.
    Fault(BError),
    /// A signal-started shutdown reached `lifecycle.drain_timeout` with services running.
    DrainTimedOut,
    /// A second signal cut the shutdown short.
    Aborted(Signal),
}

impl Outcome {
    /// A short name, as logged in `outcome`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Outcome::Stopped => "stopped",
            Outcome::StartupFailed(_) => "startup-failed",
            Outcome::Fault(_) => "fault",
            Outcome::DrainTimedOut => "drain-timed-out",
            Outcome::Aborted(_) => "aborted",
        }
    }
}

/// Something that happened during a run.
#[derive(Debug)]
pub(crate) enum Input {
    /// The service with this index called `ready()`.
    Ready(usize),
    /// The service with this index ended.
    Exited(usize, Exit),
    /// A signal arrived.
    Signal(Signal),
    /// A timer fired.
    Timer(Timer),
}

/// How a service ended.
#[derive(Debug)]
pub(crate) enum Exit {
    Returned,
    Failed(BError),
    Panicked(String),
}

/// The timers of a run, each started by the state machine.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Timer {
    /// `lifecycle.startup_timeout`, from the start.
    Startup,
    /// `lifecycle.drain_delay`, from SIGTERM.
    DrainDelay,
    /// `lifecycle.drain_timeout`, from the moment services are asked to stop.
    Deadline,
}

/// Starts services, watches them and stops them, following the table in
/// `docs/architecture.md`.
pub struct Supervisor {
    services: Vec<Box<dyn Service>>,
    settings: LifecycleSettings,
    phases: PhaseWatch,
}

impl Supervisor {
    /// A supervisor with these timings, reporting its phase through `phases`.
    #[must_use]
    pub fn new(settings: &LifecycleSettings, phases: PhaseWatch) -> Self {
        Supervisor {
            services: Vec::new(),
            settings: settings.clone(),
            phases,
        }
    }

    /// Adds a service.
    #[must_use]
    pub fn with(mut self, service: impl Service) -> Self {
        self.services.push(Box::new(service));
        self
    }

    /// Runs every service until a signal or a fault ends the run, and says how it ended.
    /// Services still running at the end are aborted.
    pub async fn run(mut self, signals: impl SignalSource) -> Outcome {
        let started = tokio::time::Instant::now();
        let (events, mut inputs) = mpsc::unbounded_channel();
        let (stop_frontline, frontline) = watch::channel(false);
        let (stop_background, background) = watch::channel(false);
        let mut services = Vec::new();
        let mut handles = Vec::new();
        for (index, service) in std::mem::take(&mut self.services).into_iter().enumerate() {
            let kind = service.kind();
            services.push((service.name(), kind));
            let shutdown = match kind {
                ServiceKind::Frontline => frontline.clone(),
                ServiceKind::Background => background.clone(),
            };
            let ctx = ServiceContext {
                index,
                events: events.clone(),
                shutdown,
            };
            let task = tokio::spawn(service.run(ctx));
            handles.push(task.abort_handle());
            let events = events.clone();
            tokio::spawn(async move {
                let exit = match task.await {
                    Ok(Ok(())) => Exit::Returned,
                    Ok(Err(error)) => Exit::Failed(error),
                    Err(error) => Exit::Panicked(panic_text(error)),
                };
                events.send(Input::Exited(index, exit)).ok();
            });
        }
        let forwarder = tokio::spawn(forward(signals, events.clone()));
        let reason = "the process started";
        tracing::info!(
            phase = "starting",
            reason,
            services = services.len(),
            "phase changed"
        );
        let mut state = State::new(services.clone());
        self.start(Timer::Startup, &events);
        // `events` lives until the end of this function, so the channel never closes.
        while let Some(input) = inputs.recv().await {
            match &input {
                Input::Exited(index, exit) => log_exit(services[*index].0, exit),
                Input::Signal(Signal::Hangup) => {
                    tracing::warn!(signal = "SIGHUP", "reloading is not supported; ignored");
                }
                _ => {}
            }
            for effect in state.step(input) {
                match effect {
                    Effect::Phase(Phase::Running, reason) => {
                        self.phases.set(Phase::Running);
                        // Seconds from the start of the run until every service was ready.
                        let startup = started.elapsed().as_secs_f64();
                        tracing::info!(
                            phase = "running",
                            reason,
                            startup.duration = startup,
                            "phase changed"
                        );
                    }
                    Effect::Phase(phase, reason) => {
                        self.phases.set(phase);
                        tracing::info!(phase = phase.as_str(), reason, "phase changed");
                        // A failing or panicking service was logged when it exited; what the
                        // supervisor finds itself is logged here, once.
                        let own = state.failure().filter(|error| {
                            [STARTUP_TIMED_OUT, SERVICE_EXITED].contains(error.etype())
                        });
                        if let Some(error) = own.filter(|_| phase == Phase::Stopping) {
                            log_error!(tracing::Level::ERROR, error, "run failed");
                        }
                    }
                    Effect::Stop(ServiceKind::Frontline) => {
                        stop_frontline.send_replace(true);
                    }
                    Effect::Stop(ServiceKind::Background) => {
                        stop_background.send_replace(true);
                    }
                    Effect::Start(timer) => self.start(timer, &events),
                    Effect::Finish(outcome) => {
                        let running = state.running().join(",");
                        match &outcome {
                            _ if running.is_empty() => {}
                            Outcome::Aborted(signal) => {
                                let signal = signal.name();
                                tracing::warn!(signal, services = running, "shutdown cut short");
                            }
                            _ => tracing::warn!(services = running, "drain timed out"),
                        }
                        handles.iter().for_each(tokio::task::AbortHandle::abort);
                        forwarder.abort();
                        self.phases.set(Phase::Stopped);
                        let name = outcome.name();
                        tracing::info!(phase = "stopped", outcome = name, "phase changed");
                        return outcome;
                    }
                }
            }
        }
        Outcome::Stopped
    }

    /// Starts a timer that sends its input when it fires.
    fn start(&self, timer: Timer, events: &mpsc::UnboundedSender<Input>) {
        let after = match timer {
            Timer::Startup => self.settings.startup_timeout,
            Timer::DrainDelay => self.settings.drain_delay,
            Timer::Deadline => self.settings.drain_timeout,
        };
        let events = events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            events.send(Input::Timer(timer)).ok();
        });
    }
}

/// Passes every signal on until the run ends.
async fn forward(mut signals: impl SignalSource, events: mpsc::UnboundedSender<Input>) {
    loop {
        let signal = signals.next().await;
        tracing::info!(signal = signal.name(), "signal received");
        if events.send(Input::Signal(signal)).is_err() {
            return;
        }
    }
}

/// Logs a failed or panicked service once, where the supervisor learns of it.
fn log_exit(name: &'static str, exit: &Exit) {
    match exit {
        Exit::Returned => tracing::info!(service.name = name, "service returned"),
        Exit::Failed(error) => {
            log_error!(
                tracing::Level::ERROR,
                error,
                service.name = name,
                "service failed"
            );
        }
        Exit::Panicked(message) => {
            tracing::error!(
                service.name = name,
                panic = message.as_str(),
                "service panicked"
            );
        }
    }
}

/// The message of a panicked task.
fn panic_text(error: JoinError) -> String {
    let Ok(payload) = error.try_into_panic() else {
        return "cancelled".to_string();
    };
    let payload: &(dyn Any + Send) = &*payload;
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked".to_string())
}
