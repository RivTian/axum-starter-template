use super::*;
use crate::config::{Config, LoadContext};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use tokio::sync::oneshot;

fn config(split: bool) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("service.toml");
    let mut text = "[runtime]\nworker_threads=1\nmax_blocking_threads=1\n".to_owned();
    if split {
        text += "[ticker]\nruntime='compute'\n[http]\nruntime='network'\n[runtime.extra.compute]\nworker_threads=1\nmax_blocking_threads=1\n[runtime.extra.network]\nworker_threads=1\nmax_blocking_threads=1\n";
    }
    std::fs::write(&path, text).unwrap();
    crate::config::load(&LoadContext::new(path, dir.path(), BTreeMap::new(), 1).unwrap()).unwrap()
}

#[test]
fn default_topology_allocates_no_extra_executor_collection_or_forwarders() {
    let cfg = config(false);
    let mut runtimes = RuntimeSet::build(&cfg.boot).unwrap();
    assert!(runtimes.extra.is_empty());
    assert_eq!(runtimes.extra.capacity(), 0);
    let executors = runtimes.executors();
    assert!(executors.extra.is_empty());
    assert_eq!(executors.extra.capacity(), 0);
    assert!(matches!(
        executors.resolve(&Binding::Main).unwrap().name,
        Cow::Borrowed("main")
    ));
    assert!(executors.resolve(&Binding::Extra("absent".into())).is_err());
    assert!(!runtimes.shutdown(Instant::now() + Duration::from_secs(2)));
}

#[test]
fn one_worker_extras_drive_tasks_without_an_extra_block_on_and_keep_named_exits() {
    let cfg = config(true);
    let mut runtimes = RuntimeSet::build(&cfg.boot).unwrap();
    let executors = runtimes.executors();
    runtimes.block_on(async {
        let target = executors.resolve(&cfg.boot.ticker_runtime).unwrap();
        let (tx, rx) = oneshot::channel();
        let mut supervisor = crate::supervisor::TaskSupervisor::new();
        supervisor
            .spawn_on("ticker", target.handle, target.name, async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tx.send(std::thread::current().name().unwrap().to_owned())
                    .unwrap();
                Ok(())
            })
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), rx)
                .await
                .unwrap()
                .unwrap(),
            "service-compute"
        );
        let exit = supervisor.next_exit().await.unwrap();
        assert_eq!(exit.name, "ticker");
        assert_eq!(exit.runtime, "compute");
        assert!(exit.kind.returned());
    });
    assert!(!runtimes.shutdown(Instant::now() + Duration::from_secs(2)));
}

#[test]
fn invalid_programmatic_topology_is_rejected_before_any_runtime_is_constructed() {
    let mut cfg = config(false);
    cfg.boot.extra_runtimes.insert(
        "unused".into(),
        RuntimeSpec {
            worker_threads: 1,
            max_blocking_threads: 1,
        },
    );
    let called = AtomicUsize::new(0);
    let result = RuntimeSet::build_with(&cfg.boot, |name, spec| {
        called.fetch_add(1, Ordering::SeqCst);
        create_runtime(name, spec)
    });
    assert!(matches!(result, Err(BuildError::Invalid(_))));
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn partial_build_failure_closes_constructed_runtimes_and_preserves_the_original_error() {
    struct Dropped(String, mpsc::Sender<String>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.1.send(self.0.clone()).unwrap();
        }
    }
    let cfg = config(true);
    let (drop_tx, drop_rx) = mpsc::channel();
    let mut created = Vec::new();
    let result = RuntimeSet::build_with(&cfg.boot, |name, spec| {
        if name == "network" {
            return Err(std::io::Error::other("controlled runtime build failure"));
        }
        created.push(name.to_owned());
        let runtime = create_runtime(name, spec)?;
        let (started_tx, started_rx) = mpsc::channel();
        let guard = Dropped(name.to_owned(), drop_tx.clone());
        runtime.spawn(async move {
            let _guard = guard;
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        Ok(runtime)
    });
    match result {
        Err(BuildError::Create {
            name,
            source,
            cleanup_exhausted,
        }) => {
            assert_eq!(name, "network");
            assert_eq!(source.to_string(), "controlled runtime build failure");
            assert!(!cleanup_exhausted);
        }
        _ => panic!("expected the named construction failure"),
    }
    assert_eq!(created, ["main", "compute"]);
    assert_eq!(
        drop_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        "compute"
    );
    assert_eq!(
        drop_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        "main"
    );
}

struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}

#[test]
fn all_runtime_shutdowns_share_one_remaining_budget_not_n_full_timeouts() {
    let cfg = config(true);
    let mut runtimes = RuntimeSet::build(&cfg.boot).unwrap();
    let executors = runtimes.executors();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Release(gate.clone());
    let (started_tx, started_rx) = mpsc::channel();
    let mut jobs = Vec::new();
    for binding in [
        Binding::Main,
        Binding::Extra("compute".into()),
        Binding::Extra("network".into()),
    ] {
        let gate = gate.clone();
        let started_tx = started_tx.clone();
        jobs.push(
            executors
                .resolve(&binding)
                .unwrap()
                .handle
                .spawn_blocking(move || {
                    started_tx.send(()).unwrap();
                    let (lock, wake) = &*gate;
                    let mut allowed = lock.lock().unwrap();
                    while !*allowed {
                        allowed = wake.wait(allowed).unwrap();
                    }
                }),
        );
    }
    for _ in 0..3 {
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    let deadline = Instant::now() + Duration::from_millis(40);
    let mut calls = Vec::new();
    let exhausted = runtimes.shutdown_with(deadline, |name, runtime, remaining| {
        calls.push((name.to_owned(), remaining));
        runtime.shutdown_timeout(remaining);
    });
    // Free the deliberate blocking work even if an assertion below fails.
    drop(release);
    let waiter = Builder::new_current_thread().enable_all().build().unwrap();
    waiter.block_on(async {
        for job in jobs {
            tokio::time::timeout(Duration::from_secs(2), job)
                .await
                .unwrap()
                .unwrap();
        }
    });
    assert!(exhausted);
    assert_eq!(
        calls
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["network", "compute", "main"]
    );
    assert!(calls[0].1 <= Duration::from_millis(40));
    assert_eq!(calls[1].1, Duration::ZERO);
    assert_eq!(calls[2].1, Duration::ZERO);
    assert!(runtimes.main.is_none() && runtimes.extra.is_empty());
}

#[test]
fn a_repeated_shutdown_cannot_refresh_the_original_cutoff() {
    let cfg = config(false);
    let mut runtimes = RuntimeSet::build(&cfg.boot).unwrap();
    let first = Instant::now() + Duration::from_secs(1);
    runtimes.shutdown(first);
    runtimes.shutdown(first + Duration::from_secs(60));
    assert_eq!(runtimes.shutdown_deadline, Some(first));
}
