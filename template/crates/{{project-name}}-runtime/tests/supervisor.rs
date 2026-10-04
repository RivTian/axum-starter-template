//! The supervisor end to end, with scripted services and hand-sent signals on paused time.
//! The table itself is tested cell by cell in the crate; these tests check that the driver
//! carries it out. The last test runs scenarios many times on several threads, to show the
//! outcome does not depend on scheduling.

use std::time::Duration;

use svc_runtime::phase::PhaseWatch;
use svc_runtime::service::ServiceKind::{Background, Frontline};
use svc_runtime::settings::LifecycleSettings;
use svc_runtime::signal::Signal;
use svc_runtime::supervisor::{Outcome, Supervisor};
use svc_test_utils::services::{ScriptedService, Step};
use svc_test_utils::signals::FakeSignals;
use svc_util::error::ErrorType;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// Runs the services, sending each signal at its time, and returns the outcome's name.
async fn run(services: Vec<ScriptedService>, signals: &[(u64, Signal)]) -> &'static str {
    let (source, sender) = FakeSignals::new();
    for (at, signal) in signals.iter().copied() {
        let sender = sender.clone();
        tokio::spawn(async move {
            tokio::time::sleep(secs(at)).await;
            sender.send(signal);
        });
    }
    let mut supervisor = Supervisor::new(&LifecycleSettings::default(), PhaseWatch::new());
    for service in services {
        supervisor = supervisor.with(service);
    }
    supervisor.run(source).await.name()
}

fn http(steps: Vec<Step>) -> ScriptedService {
    ScriptedService::new("http", Frontline, steps)
}

fn log(steps: Vec<Step>) -> ScriptedService {
    ScriptedService::new("log", Background, steps)
}

fn serving() -> Vec<Step> {
    vec![Step::Ready, Step::UntilShutdown]
}

#[tokio::test(start_paused = true)]
async fn sigterm_drains_then_stops_frontline_before_background() {
    let front = http(vec![Step::Ready, Step::UntilShutdown, Step::Sleep(secs(1))]);
    let back = log(serving());
    let (front_journal, back_journal) = (front.journal(), back.journal());
    let started = tokio::time::Instant::now();
    let watcher = tokio::spawn(async move {
        // SIGTERM at 1s, the drain delay ends at 6s, http needs one more second.
        tokio::time::sleep_until(started + secs(6) + Duration::from_millis(500)).await;
        (front_journal.entries(), back_journal.entries())
    });
    assert_eq!(
        run(vec![front, back], &[(1, Signal::Terminate)]).await,
        "stopped"
    );
    let Ok((front_seen, back_seen)) = watcher.await else {
        return;
    };
    assert_eq!(front_seen, ["started", "ready", "shutdown requested"]);
    assert_eq!(back_seen, ["started", "ready"]);
}

#[tokio::test(start_paused = true)]
async fn a_failure_before_ready_fails_the_startup() {
    let front = http(vec![Step::Sleep(secs(1)), Step::Fail(ErrorType::BindError)]);
    assert_eq!(
        run(vec![front, log(serving())], &[]).await,
        "startup-failed"
    );
}

#[tokio::test(start_paused = true)]
async fn a_background_job_that_finishes_early_does_not_fail_the_startup() {
    let job = log(vec![Step::Ready, Step::Return]);
    let front = http(vec![Step::Sleep(secs(1)), Step::Ready, Step::UntilShutdown]);
    assert_eq!(
        run(vec![front, job], &[(2, Signal::Interrupt)]).await,
        "stopped"
    );
}

#[tokio::test(start_paused = true)]
async fn not_ready_in_time_fails_the_startup() {
    let slow = http(vec![
        Step::Sleep(secs(60)),
        Step::Ready,
        Step::UntilShutdown,
    ]);
    assert_eq!(run(vec![slow, log(serving())], &[]).await, "startup-failed");
}

#[tokio::test(start_paused = true)]
async fn a_panic_while_running_is_a_fault() {
    let back = log(vec![Step::Ready, Step::Sleep(secs(1)), Step::Panic]);
    assert_eq!(run(vec![http(serving()), back], &[]).await, "fault");
}

#[tokio::test(start_paused = true)]
async fn a_service_that_ignores_shutdown_is_abandoned_at_the_deadline() {
    let stuck = http(vec![Step::Ready, Step::Sleep(secs(3600))]);
    let outcome = run(vec![stuck, log(serving())], &[(1, Signal::Terminate)]).await;
    assert_eq!(outcome, "drain-timed-out");
}

#[tokio::test(start_paused = true)]
async fn a_second_signal_aborts_the_shutdown() {
    let stuck = http(vec![Step::Ready, Step::Sleep(secs(3600))]);
    let signals = [(1, Signal::Terminate), (2, Signal::Interrupt)];
    let started = tokio::time::Instant::now();
    let outcome = run(vec![stuck, log(serving())], &signals).await;
    assert_eq!(outcome, "aborted");
    assert_eq!(started.elapsed(), secs(2));
}

#[tokio::test(start_paused = true)]
async fn the_outcome_carries_the_first_error() {
    let front = http(vec![
        Step::Ready,
        Step::Sleep(secs(1)),
        Step::Fail(ErrorType::ReadError),
    ]);
    let late = log(vec![
        Step::Ready,
        Step::UntilShutdown,
        Step::Fail(ErrorType::WriteError),
    ]);
    let mut supervisor = Supervisor::new(&LifecycleSettings::default(), PhaseWatch::new());
    supervisor = supervisor.with(front).with(late);
    let (source, _sender) = FakeSignals::new();
    let outcome = supervisor.run(source).await;
    let chain = match &outcome {
        Outcome::Fault(error) => error.fields().chain,
        other => other.name().to_string(),
    };
    assert_eq!(chain, "ServiceFailed,ReadError");
}

/// Runs the services on a multi-threaded runtime, without paused time, `runs` times, and
/// returns every outcome seen.
async fn outcomes(make: impl Fn() -> Vec<ScriptedService>, runs: usize) -> Vec<&'static str> {
    let mut seen = Vec::new();
    for _ in 0..runs {
        let (source, _sender) = FakeSignals::new();
        let mut supervisor = Supervisor::new(&LifecycleSettings::default(), PhaseWatch::new());
        for service in make() {
            supervisor = supervisor.with(service);
        }
        let outcome = supervisor.run(source).await.name();
        if !seen.contains(&outcome) {
            seen.push(outcome);
        }
    }
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outcome_does_not_depend_on_scheduling() {
    // A service that is ready and fails at once, while another one becomes ready.
    let ready_then_fail = || {
        vec![
            http(vec![Step::Ready, Step::Fail(ErrorType::AcceptError)]),
            log(serving()),
        ]
    };
    assert_eq!(outcomes(ready_then_fail, 300).await, ["fault"]);
    // A service that fails before it is ready, while another one becomes ready.
    let fail_before_ready = || vec![http(vec![Step::Fail(ErrorType::BindError)]), log(serving())];
    assert_eq!(outcomes(fail_before_ready, 300).await, ["startup-failed"]);
}

#[tokio::test(start_paused = true)]
async fn a_drain_timeout_is_logged_once() {
    let (logs, _guard) = svc_test_utils::logs::CapturedLogs::start();
    let stuck = http(vec![Step::Ready, Step::Sleep(secs(3600))]);
    let outcome = run(vec![stuck, log(serving())], &[(1, Signal::Interrupt)]).await;
    assert_eq!(outcome, "drain-timed-out");
    let warnings = logs.with_message("drain timed out");
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].get("services").and_then(|s| s.as_str()),
        Some("http,log")
    );
}
