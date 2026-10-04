//! One test per cell of the supervisor's table; each starts from a state reached through
//! the same inputs the driver would give.

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
fn a_service_that_ends_before_it_is_ready_fails_the_startup() {
    for exit in [Exit::Returned, failed(), Exit::Panicked("boom".into())] {
        let mut s = state();
        s.step(Input::Ready(1));
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
fn a_ready_service_that_fails_while_others_start_is_a_fault() {
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
            ["finish fault"]
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
fn a_service_that_returns_while_draining_is_recorded() {
    let mut s = running();
    s.step(Input::Signal(Terminate));
    assert_eq!(
        step(&mut s, Input::Exited(1, Exit::Returned)),
        Vec::<String>::new()
    );
    assert_eq!(
        step(&mut s, Input::Timer(Timer::DrainDelay)),
        ["stopping", "timer deadline", "stop frontline"]
    );
    assert_eq!(
        step(&mut s, Input::Exited(0, Exit::Returned)),
        ["stop background", "finish stopped"]
    );
}

#[test]
fn sighup_while_stopping_changes_nothing() {
    let mut s = running();
    s.step(Input::Signal(Interrupt));
    assert_eq!(step(&mut s, Input::Signal(Hangup)), Vec::<String>::new());
    assert_eq!(step(&mut s, Input::Signal(Terminate)), ["finish aborted"]);
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
