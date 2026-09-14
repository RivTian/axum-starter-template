use super::*;
use crate::config::LoadContext;
use service_core::{TickerInterval, config::HotConfig};
use std::collections::BTreeMap;
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;

pub(super) fn candidate() -> Config {
    let context = LoadContext::new(
        std::env::temp_dir().join("config/service.toml"),
        &std::env::temp_dir(),
        BTreeMap::new(),
        2,
    )
    .unwrap();
    super::super::load::parse("", &context).unwrap()
}
fn hot(millis: u64) -> HotConfig {
    HotConfig {
        ticker: TickerInterval::try_from(Duration::from_millis(millis)).unwrap(),
    }
}

#[tokio::test]
async fn typed_cold_equality_rejects_mixed_changes_without_publishing_anything() {
    let initial = candidate();
    let mut mixed = initial.clone();
    mixed.hot = hot(50);
    mixed.boot.worker_threads += 1;
    let mut reload = ReloadController::new(&initial, move || Ok(mixed.clone()));
    let mut reader = reload.reader();
    reload.request();
    let completion = reload.next_completion().await;
    let report = reload.complete(completion).unwrap();
    assert_eq!(report.outcome, Outcome::RequiresRestart);
    assert_eq!(report.fields, ["runtime.worker_threads"]);
    assert_eq!(reader.current().generation, 1);
    assert_eq!(reader.current().config, initial.hot);
    assert!(
        timeout(Duration::from_millis(10), reader.changed())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn valid_hot_changes_publish_once_and_no_change_or_errors_keep_the_version() {
    let initial = candidate();
    let source = Arc::new(Mutex::new(initial.clone()));
    let source_for_loader = source.clone();
    let mut reload = ReloadController::new(&initial, move || {
        Ok(source_for_loader.lock().unwrap().clone())
    });
    let reader = reload.reader();
    reload.request();
    let completion = reload.next_completion().await;
    assert_eq!(
        reload.complete(completion).unwrap().outcome,
        Outcome::NoChange
    );
    source.lock().unwrap().hot = hot(50);
    reload.request();
    let completion = reload.next_completion().await;
    assert_eq!(
        reload.complete(completion).unwrap().outcome,
        Outcome::Published
    );
    assert_eq!(reader.current().generation, 2);
    assert_eq!(reader.current().config, hot(50));
    let mut bad =
        ReloadController::new(&initial, || Err(ConfigError::new("file", "test rejection")));
    bad.request();
    let completion = bad.next_completion().await;
    assert_eq!(bad.complete(completion).unwrap().outcome, Outcome::Invalid);
    assert_eq!(bad.reader().current().generation, 1);
}

struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
fn held_loader(
    initial: &Config,
) -> (
    ReloadController,
    Release,
    oneshot::Receiver<()>,
    Arc<AtomicUsize>,
) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(gate.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let mut next = initial.clone();
    next.hot = hot(50);
    let reload = ReloadController::new(initial, move || {
        let call = count.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            tx.lock().unwrap().take().unwrap().send(()).unwrap();
            let (lock, wake) = &*gate;
            let mut allowed = lock.lock().unwrap();
            while !*allowed {
                allowed = wake.wait(allowed).unwrap();
            }
        }
        Ok(next.clone())
    });
    (reload, release, rx, calls)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_job_keeps_its_slot_and_a_burst_creates_only_one_followup() {
    let mut initial = candidate();
    initial.boot.reload_timeout = Duration::from_millis(20);
    let (mut reload, release, started, calls) = held_loader(&initial);
    reload.request();
    timeout(Duration::from_secs(1), started)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..100 {
        reload.request();
    }
    assert!(reload.pending);
    assert_eq!(reload.jobs.len(), 1);
    tokio::time::sleep_until(reload.deadline().unwrap()).await;
    assert_eq!(reload.expire_if_due().unwrap().outcome, Outcome::Timeout);
    assert!(reload.expire_if_due().is_none());
    assert!(reload.deadline().is_none());
    for _ in 0..100 {
        reload.request();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(reload.jobs.len(), 1);
    drop(release);
    let completion = timeout(Duration::from_secs(1), reload.next_completion())
        .await
        .unwrap();
    assert_eq!(
        reload.complete(completion).unwrap().outcome,
        Outcome::DiscardedExpired
    );
    assert_eq!(reload.reader().current().generation, 1);
    let completion = timeout(Duration::from_secs(1), reload.next_completion())
        .await
        .unwrap();
    assert_eq!(
        reload.complete(completion).unwrap().outcome,
        Outcome::Published
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!reload.is_busy());
    assert!(!reload.pending);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_suppresses_a_late_result_and_never_starts_the_pending_job() {
    let initial = candidate();
    let (mut reload, release, started, calls) = held_loader(&initial);
    let reader = reload.reader();
    reload.request();
    started.await.unwrap();
    reload.request();
    reload.stop();
    reload.request();
    assert!(!reload.pending);
    assert!(reload.deadline().is_none());
    drop(release);
    let completion = timeout(Duration::from_secs(1), reload.next_completion())
        .await
        .unwrap();
    assert_eq!(
        reload.complete(completion).unwrap().outcome,
        Outcome::DiscardedShutdown
    );
    assert_eq!(reader.current().generation, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_expired_completion_is_rejected_even_before_the_timeout_branch_was_polled() {
    let mut initial = candidate();
    initial.boot.reload_timeout = Duration::from_millis(1);
    let mut next = initial.clone();
    next.hot = hot(50);
    let mut reload = ReloadController::new(&initial, move || Ok(next.clone()));
    reload.request();
    let completion = reload.next_completion().await;
    tokio::time::sleep_until(reload.deadline().unwrap()).await;
    assert_eq!(
        reload.complete(completion).unwrap().outcome,
        Outcome::Timeout
    );
    assert_eq!(reload.reader().current().generation, 1);
}

#[tokio::test]
async fn a_loader_panic_is_not_an_invalid_configuration_or_an_automatic_retry() {
    let initial = candidate();
    let mut reload = ReloadController::new(&initial, || panic!("controlled loader panic"));
    reload.request();
    reload.request();
    let completion = reload.next_completion().await;
    assert_eq!(
        reload.complete(completion).unwrap_err(),
        ReloadFailure::Panic
    );
    assert!(reload.closing);
    assert!(!reload.pending);
    assert!(!reload.is_busy());
}

#[test]
fn closed_commit_capability_never_increments_the_snapshot() {
    let initial = candidate();
    let mut next = initial.clone();
    next.hot = hot(50);
    let mut reload = ReloadController::new(&initial, || unreachable!());
    reload.stop();
    assert_eq!(reload.commit(next).outcome, Outcome::DiscardedShutdown);
    assert_eq!(reload.reader().current().generation, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panic_after_timeout_is_still_a_control_path_failure() {
    let mut initial = candidate();
    initial.boot.reload_timeout = Duration::from_millis(20);
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(gate.clone());
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let mut reload = ReloadController::new(&initial, move || {
        tx.lock().unwrap().take().unwrap().send(()).unwrap();
        let (lock, wake) = &*gate;
        let mut allowed = lock.lock().unwrap();
        while !*allowed {
            allowed = wake.wait(allowed).unwrap();
        }
        panic!("controlled panic after reload expiry")
    });
    reload.request();
    rx.await.unwrap();
    tokio::time::sleep_until(reload.deadline().unwrap()).await;
    assert_eq!(reload.expire_if_due().unwrap().outcome, Outcome::Timeout);
    drop(release);
    let completion = timeout(Duration::from_secs(1), reload.next_completion())
        .await
        .unwrap();
    assert_eq!(
        reload.complete(completion).unwrap_err(),
        ReloadFailure::Panic
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_blocking_work_is_not_falsely_reported_joined_when_aborted() {
    let initial = candidate();
    let (mut reload, release, started, calls) = held_loader(&initial);
    reload.request();
    started.await.unwrap();
    reload.stop();
    reload.abort_remaining();
    assert!(
        timeout(Duration::from_millis(20), reload.next_completion())
            .await
            .is_err()
    );
    assert!(reload.is_busy());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(release);
    let completion = timeout(Duration::from_secs(1), reload.next_completion())
        .await
        .unwrap();
    assert!(reload.discard_for_shutdown(completion));
    assert!(!reload.is_busy());
    assert_eq!(reload.reader().current().generation, 1);
}

#[tokio::test]
async fn adding_or_rebinding_runtime_topology_is_cold_even_with_a_hot_period_change() {
    use crate::config::{Binding, RuntimeSpec};
    let initial = candidate();
    let mut changed = initial.clone();
    changed.boot.extra_runtimes.insert(
        "compute".into(),
        RuntimeSpec {
            worker_threads: 1,
            max_blocking_threads: 1,
        },
    );
    changed.boot.ticker_runtime = Binding::Extra("compute".into());
    changed.hot = hot(50);
    changed.boot.validate_runtimes().unwrap();
    let mut reload = ReloadController::new(&initial, move || Ok(changed.clone()));
    let reader = reload.reader();
    reload.request();
    let completion = reload.next_completion().await;
    let result = reload.complete(completion).unwrap();
    assert_eq!(result.outcome, Outcome::RequiresRestart);
    assert_eq!(result.fields, ["runtime.extra", "ticker.runtime"]);
    assert_eq!(reader.current().generation, 1);
    assert_eq!(reader.current().config, initial.hot);
}
