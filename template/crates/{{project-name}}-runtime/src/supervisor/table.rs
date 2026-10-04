//! The supervisor's decisions as a pure state machine: one input in, the effects out. Nothing
//! here waits, spawns or reads a clock, so every cell of the table is tested directly.

use svc_util::prelude::*;

use super::{Exit, Input, Outcome, Timer};
use crate::error::{SERVICE_EXITED, SERVICE_FAILED, SERVICE_PANICKED, STARTUP_TIMED_OUT};
use crate::phase::Phase;
use crate::service::ServiceKind;
use crate::signal::Signal;

/// What the driver does for the state machine.
#[derive(Debug)]
pub(super) enum Effect {
    /// The phase changed, for this reason.
    Phase(Phase, String),
    /// Ask the services of this kind to stop.
    Stop(ServiceKind),
    /// Start this timer.
    Start(Timer),
    /// The run is over.
    Finish(Outcome),
}

/// Why the services are being stopped. The first reason wins, except that a service failing
/// during a signal-started shutdown turns it into a fault.
#[derive(Debug)]
enum Reason {
    Startup(BError),
    Fault(BError),
    Signal(Signal),
}

/// The state of a run.
#[derive(Debug)]
pub(super) struct State {
    services: Vec<(&'static str, ServiceKind)>,
    ready: Vec<bool>,
    alive: Vec<bool>,
    phase: Phase,
    reason: Option<Reason>,
    signals: usize,
    background_stopped: bool,
    timed_out: bool,
}

impl State {
    /// A run of these services, every one started and none ready.
    pub(super) fn new(services: Vec<(&'static str, ServiceKind)>) -> Self {
        let count = services.len();
        State {
            services,
            ready: vec![false; count],
            alive: vec![true; count],
            phase: Phase::Starting,
            reason: None,
            signals: 0,
            background_stopped: false,
            timed_out: false,
        }
    }

    /// Handles one input and says what to do.
    pub(super) fn step(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        match input {
            Input::Ready(i) => {
                self.ready[i] = true;
                self.enter_running_if_ready(&mut out);
            }
            Input::Exited(i, exit) => self.exited(i, exit, &mut out),
            Input::Signal(signal) => self.signal(signal, &mut out),
            Input::Timer(Timer::Startup) if self.phase == Phase::Starting => {
                let waiting: Vec<&str> = (self.services.iter().zip(&self.ready))
                    .filter(|(_, ready)| !**ready)
                    .map(|((name, _), _)| *name)
                    .collect();
                let error = Error::explain(
                    STARTUP_TIMED_OUT,
                    format!("not ready in time: {}", waiting.join(", ")),
                );
                self.stop(Reason::Startup(error), &mut out);
            }
            Input::Timer(Timer::DrainDelay) if self.phase == Phase::Draining => {
                self.enter_stopping(&mut out);
            }
            Input::Timer(Timer::Deadline) if self.phase == Phase::Stopping => {
                self.timed_out = true;
                out.push(Effect::Finish(self.outcome()));
            }
            Input::Timer(_) => {}
        }
        out
    }

    /// The names of the services still running.
    pub(super) fn running(&self) -> Vec<&'static str> {
        (self.services.iter().zip(&self.alive))
            .filter(|(_, alive)| **alive)
            .map(|((name, _), _)| *name)
            .collect()
    }

    fn exited(&mut self, i: usize, exit: Exit, out: &mut Vec<Effect>) {
        let (name, kind) = self.services[i];
        self.alive[i] = false;
        let error = match exit {
            Exit::Returned => None,
            Exit::Failed(error) => Some(Error::because(
                SERVICE_FAILED,
                format!("service {name} failed"),
                error,
            )),
            Exit::Panicked(message) => Some(Error::explain(
                SERVICE_PANICKED,
                format!("service {name} panicked: {message}"),
            )),
        };
        // A background service that was ready may finish; anything else returning is wrong.
        let finished = error.is_none() && kind == ServiceKind::Background && self.ready[i];
        match self.phase {
            Phase::Starting | Phase::Running if finished => {
                tracing::info!(service.name = name, "service finished");
                self.enter_running_if_ready(out);
            }
            Phase::Starting | Phase::Running => {
                let error = error.unwrap_or_else(|| {
                    Error::explain(SERVICE_EXITED, format!("service {name} returned early"))
                });
                let reason = if self.phase == Phase::Starting {
                    Reason::Startup(error)
                } else {
                    Reason::Fault(error)
                };
                self.stop(reason, out);
            }
            Phase::Draining | Phase::Stopping | Phase::Stopped => {
                if let Some(error) = error {
                    if matches!(self.reason, Some(Reason::Signal(_)) | None) {
                        self.reason = Some(Reason::Fault(error));
                    }
                    self.enter_stopping(out);
                }
                self.progress(out);
            }
        }
    }

    fn signal(&mut self, signal: Signal, out: &mut Vec<Effect>) {
        if signal == Signal::Hangup {
            tracing::warn!(
                signal = signal.name(),
                "reloading is not supported; ignored"
            );
            return;
        }
        self.signals += 1;
        if self.signals > 1 {
            out.push(Effect::Finish(Outcome::Aborted(signal)));
            return;
        }
        match self.phase {
            Phase::Running if signal == Signal::Terminate => {
                self.reason = Some(Reason::Signal(signal));
                self.phase = Phase::Draining;
                out.push(Effect::Phase(Phase::Draining, signal.name().to_string()));
                out.push(Effect::Start(Timer::DrainDelay));
            }
            Phase::Starting | Phase::Running => self.stop(Reason::Signal(signal), out),
            Phase::Draining | Phase::Stopping | Phase::Stopped => {
                tracing::info!(signal = signal.name(), "already stopping");
            }
        }
    }

    fn enter_running_if_ready(&mut self, out: &mut Vec<Effect>) {
        if self.phase == Phase::Starting && self.ready.iter().all(|ready| *ready) {
            self.phase = Phase::Running;
            out.push(Effect::Phase(
                Phase::Running,
                "every service is ready".into(),
            ));
        }
    }

    fn stop(&mut self, reason: Reason, out: &mut Vec<Effect>) {
        self.reason.get_or_insert(reason);
        self.enter_stopping(out);
    }

    fn enter_stopping(&mut self, out: &mut Vec<Effect>) {
        if matches!(self.phase, Phase::Stopping | Phase::Stopped) {
            return;
        }
        self.phase = Phase::Stopping;
        // The reason names the error, so that errors the supervisor makes itself, such as a
        // startup timeout, are logged too.
        let why = match &self.reason {
            Some(Reason::Startup(error) | Reason::Fault(error)) => match &error.context {
                Some(context) => context.as_str().to_string(),
                None => error.etype().as_str().to_string(),
            },
            Some(Reason::Signal(signal)) => signal.name().to_string(),
            None => "fault".to_string(),
        };
        out.push(Effect::Phase(Phase::Stopping, why));
        out.push(Effect::Start(Timer::Deadline));
        out.push(Effect::Stop(ServiceKind::Frontline));
        self.progress(out);
    }

    /// While stopping: background services stop once no frontline service runs, and the run
    /// is over once nothing runs.
    fn progress(&mut self, out: &mut Vec<Effect>) {
        if self.phase != Phase::Stopping {
            return;
        }
        let running = |kind| {
            (self.services.iter().zip(&self.alive)).any(|((_, k), alive)| *k == kind && *alive)
        };
        if !self.background_stopped && !running(ServiceKind::Frontline) {
            self.background_stopped = true;
            out.push(Effect::Stop(ServiceKind::Background));
        }
        if !self.alive.iter().any(|alive| *alive) {
            self.phase = Phase::Stopped;
            out.push(Effect::Finish(self.outcome()));
        }
    }

    fn outcome(&mut self) -> Outcome {
        match self.reason.take() {
            Some(Reason::Startup(error)) => Outcome::StartupFailed(error),
            Some(Reason::Fault(error)) => Outcome::Fault(error),
            Some(Reason::Signal(_)) | None if self.timed_out => Outcome::DrainTimedOut,
            Some(Reason::Signal(_)) | None => Outcome::Stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    //! One test per cell of the table in `docs/architecture.md`; each starts from a state
    //! reached through the same inputs the driver would give.

    use svc_util::prelude::*;

    use super::{Effect, State};
    use crate::phase::Phase;
    use crate::service::ServiceKind::{Background, Frontline};
    use crate::signal::Signal::{Hangup, Interrupt, Terminate};
    use crate::supervisor::{Exit, Input, Outcome, Timer};

    /// `http` (frontline) and `log` (background).
    fn state() -> State {
        State::new(vec![("http", Frontline), ("log", Background)])
    }

    fn running() -> State {
        let mut s = state();
        s.step(Input::Ready(0));
        s.step(Input::Ready(1));
        s
    }

    fn failed() -> Exit {
        Exit::Failed(Error::explain(ErrorType::InternalError, "broken"))
    }

    /// The effects as short words, so expectations stay readable.
    fn words(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .map(|effect| match effect {
                Effect::Phase(phase, _) => phase.to_string(),
                Effect::Stop(kind) => format!("stop {kind:?}").to_lowercase(),
                Effect::Start(timer) => format!("timer {timer:?}").to_lowercase(),
                Effect::Finish(outcome) => format!("finish {}", outcome.name()),
            })
            .collect()
    }

    fn step(s: &mut State, input: Input) -> Vec<String> {
        words(&s.step(input))
    }

    #[test]
    fn starting_runs_once_every_service_is_ready() {
        let mut s = state();
        assert_eq!(step(&mut s, Input::Ready(0)), Vec::<String>::new());
        assert_eq!(step(&mut s, Input::Ready(0)), Vec::<String>::new());
        assert_eq!(step(&mut s, Input::Ready(1)), ["running"]);
    }

    #[test]
    fn starting_fails_when_a_service_returns_fails_or_panics() {
        for exit in [Exit::Returned, failed(), Exit::Panicked("boom".into())] {
            let mut s = state();
            s.step(Input::Ready(0));
            assert_eq!(
                step(&mut s, Input::Exited(0, exit)),
                [
                    "stopping",
                    "timer deadline",
                    "stop frontline",
                    "stop background"
                ]
            );
            assert_eq!(
                step(&mut s, Input::Exited(1, Exit::Returned)),
                ["finish startup-failed"]
            );
        }
    }

    #[test]
    fn a_ready_background_service_may_finish_while_starting() {
        let mut s = state();
        s.step(Input::Ready(1));
        assert_eq!(
            step(&mut s, Input::Exited(1, Exit::Returned)),
            Vec::<String>::new()
        );
        assert_eq!(step(&mut s, Input::Ready(0)), ["running"]);
    }

    #[test]
    fn starting_fails_when_the_startup_timer_fires() {
        let mut s = state();
        s.step(Input::Ready(1));
        assert_eq!(
            step(&mut s, Input::Timer(Timer::Startup)),
            ["stopping", "timer deadline", "stop frontline"]
        );
        assert_eq!(
            step(&mut s, Input::Exited(0, Exit::Returned)),
            ["stop background"]
        );
        let effects = s.step(Input::Exited(1, Exit::Returned));
        let context = match effects.last() {
            Some(Effect::Finish(Outcome::StartupFailed(error))) => error.fields().context,
            _ => format!("{effects:?}"),
        };
        assert_eq!(context, "not ready in time: http");
    }

    #[test]
    fn a_signal_while_starting_stops_at_once() {
        for signal in [Terminate, Interrupt] {
            let mut s = state();
            assert_eq!(
                step(&mut s, Input::Signal(signal)),
                ["stopping", "timer deadline", "stop frontline"]
            );
        }
    }

    #[test]
    fn running_tolerates_a_finished_background_service_only() {
        let mut s = running();
        assert_eq!(
            step(&mut s, Input::Exited(1, Exit::Returned)),
            Vec::<String>::new()
        );
        assert_eq!(
            step(&mut s, Input::Exited(0, Exit::Returned)),
            [
                "stopping",
                "timer deadline",
                "stop frontline",
                "stop background",
                "finish fault"
            ]
        );
    }

    #[test]
    fn running_stops_as_a_fault_when_a_service_fails_or_panics() {
        for exit in [failed(), Exit::Panicked("boom".into())] {
            let mut s = running();
            assert_eq!(
                step(&mut s, Input::Exited(1, exit)),
                ["stopping", "timer deadline", "stop frontline"]
            );
            assert_eq!(
                step(&mut s, Input::Exited(0, Exit::Returned)),
                ["stop background", "finish fault"]
            );
        }
    }

    #[test]
    fn sigterm_drains_before_stopping_and_sigint_does_not() {
        let mut s = running();
        assert_eq!(
            step(&mut s, Input::Signal(Terminate)),
            ["draining", "timer draindelay"]
        );
        assert_eq!(
            step(&mut s, Input::Timer(Timer::DrainDelay)),
            ["stopping", "timer deadline", "stop frontline"]
        );
        let mut s = running();
        assert_eq!(
            step(&mut s, Input::Signal(Interrupt)),
            ["stopping", "timer deadline", "stop frontline"]
        );
    }

    #[test]
    fn a_signal_started_shutdown_ends_with_stopped() {
        let mut s = running();
        s.step(Input::Signal(Interrupt));
        assert_eq!(
            step(&mut s, Input::Exited(0, Exit::Returned)),
            ["stop background"]
        );
        assert_eq!(
            step(&mut s, Input::Exited(1, Exit::Returned)),
            ["finish stopped"]
        );
    }

    #[test]
    fn a_failure_during_a_signal_started_shutdown_is_a_fault() {
        let mut s = running();
        s.step(Input::Signal(Terminate));
        assert_eq!(
            step(&mut s, Input::Exited(1, failed())),
            ["stopping", "timer deadline", "stop frontline"]
        );
        s.step(Input::Exited(0, Exit::Returned));
        let mut s = running();
        s.step(Input::Signal(Interrupt));
        s.step(Input::Exited(0, failed()));
        assert_eq!(
            step(&mut s, Input::Exited(1, Exit::Returned)),
            ["finish fault"]
        );
    }

    #[test]
    fn the_second_signal_aborts_and_the_first_never_does() {
        let mut s = running();
        s.step(Input::Signal(Terminate));
        assert_eq!(step(&mut s, Input::Signal(Interrupt)), ["finish aborted"]);
        // A fault stops the process; the first signal after it is still the first.
        let mut s = running();
        s.step(Input::Exited(0, failed()));
        assert_eq!(step(&mut s, Input::Signal(Terminate)), Vec::<String>::new());
        assert_eq!(step(&mut s, Input::Signal(Terminate)), ["finish aborted"]);
    }

    #[test]
    fn sighup_is_ignored_in_every_phase() {
        let mut s = state();
        assert_eq!(step(&mut s, Input::Signal(Hangup)), Vec::<String>::new());
        let mut s = running();
        assert_eq!(step(&mut s, Input::Signal(Hangup)), Vec::<String>::new());
        s.step(Input::Signal(Terminate));
        assert_eq!(step(&mut s, Input::Signal(Hangup)), Vec::<String>::new());
        assert_eq!(step(&mut s, Input::Signal(Interrupt)), ["finish aborted"]);
    }

    #[test]
    fn the_deadline_ends_the_run() {
        let mut s = running();
        s.step(Input::Signal(Interrupt));
        assert_eq!(
            step(&mut s, Input::Timer(Timer::Deadline)),
            ["finish drain-timed-out"]
        );
        let mut s = running();
        s.step(Input::Exited(1, failed()));
        assert_eq!(
            step(&mut s, Input::Timer(Timer::Deadline)),
            ["finish fault"]
        );
    }

    #[test]
    fn timers_out_of_their_phase_do_nothing() {
        let mut s = running();
        for timer in [Timer::Startup, Timer::DrainDelay, Timer::Deadline] {
            assert_eq!(step(&mut s, Input::Timer(timer)), Vec::<String>::new());
        }
        assert_eq!(s.phase, Phase::Running);
    }
}
