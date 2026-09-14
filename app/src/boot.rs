//! Startup transaction and bounded cleanup. Every fallible future borrows its
//! outer resource owner; cancellation never loses a half-initialized pool.

use crate::config::reload::{self, ReloadController};
use crate::config::{BootConfig, Config, ConfigError};
use crate::rt::Executors;
use crate::shutdown::{
    Budgets, RunReport, ShutdownContext, StopCause, StorageClose, stage_deadline,
};
use crate::signals::{ControlResult, Event};
use crate::supervisor::{TaskError, TaskExit, TaskSupervisor};
use service_core::BuildInfo;
use service_core::lifecycle::{self, LifecycleHandle, LifecyclePublisher, Phase};
use service_storage::StorageOwner;
use std::future::Future;
use std::net::SocketAddr;
use std::ops::AsyncFnMut;
use tokio::sync::oneshot;
use tokio::time::{sleep_until, timeout_at};
use tokio_util::sync::CancellationToken;

struct Gate<'a> {
    writer: LifecyclePublisher,
    root: CancellationToken,
    context: &'a mut ShutdownContext,
    budget: Budgets,
    events_open: bool,
}
impl Gate<'_> {
    fn stop(&mut self, cause: StopCause) {
        self.context.begin(cause, self.budget);
        self.writer.publish(Phase::Draining);
        self.root.cancel();
        tracing::info!(
            event = "shutdown_started",
            cause = self.context.cause.as_ref().expect("recorded cause").label(),
            "service draining"
        );
    }
    fn event(&mut self, event: ControlResult) -> bool {
        match event {
            Ok(Event::Stop) => {
                self.stop(StopCause::Signal);
                true
            }
            Ok(Event::Reload) => {
                tracing::warn!(
                    event = "reload_ignored",
                    phase = "startup",
                    "reload ignored before startup commits"
                );
                false
            }
            Err(_) => {
                self.events_open = false;
                self.stop(StopCause::ControlClosed);
                true
            }
        }
    }
}
struct Resources<'a> {
    gate: Gate<'a>,
    supervisor: TaskSupervisor,
    reload: ReloadController,
    storage: Option<StorageOwner>,
    committed: bool,
    finished: bool,
}
impl Drop for Resources<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.reload.stop();
            self.gate
                .context
                .begin(StopCause::CoordinatorPanic, self.gate.budget);
            self.gate.context.emergency_unreaped = self.supervisor.pending_names();
            if self.reload.is_busy() {
                self.gate.context.emergency_unreaped.push(reload::JOB_NAME);
            }
            self.reload.abort_remaining();
            self.gate.writer.publish(Phase::Forcing);
            self.gate.root.cancel();
            self.supervisor.close_registration();
            self.supervisor.abort_remaining();
        }
    }
}

// AsyncFnMut injects the event source for tests without a Service/Signal trait
// or a permanent signal-forwarding task. Production directly awaits OS signals.
pub(crate) async fn run<C, S, L>(
    config: &Config,
    build: BuildInfo,
    context: &mut ShutdownContext,
    mut control: C,
    on_started: S,
    loader: L,
    executors: &Executors,
) -> RunReport
where
    C: AsyncFnMut() -> ControlResult,
    S: FnOnce(SocketAddr),
    L: Fn() -> Result<Config, ConfigError> + Send + Sync + 'static,
{
    let (writer, reader) = lifecycle::channel();
    let reload = ReloadController::new(config, loader);
    let mut resources = Resources {
        gate: Gate {
            writer,
            root: CancellationToken::new(),
            context,
            budget: config.boot.shutdown,
            events_open: true,
        },
        supervisor: TaskSupervisor::new(),
        reload,
        storage: None,
        committed: false,
        finished: false,
    };
    let deadline = tokio::time::Instant::now() + config.boot.startup_timeout;
    if let Some(address) = start(
        &mut resources,
        reader,
        &config.boot,
        build,
        deadline,
        &mut control,
        executors,
    )
    .await
    {
        resources.committed = true;
        resources.gate.writer.publish(Phase::Running);
        on_started(address);
        monitor(&mut resources, &mut control).await;
    }
    let report = cleanup(&mut resources, &mut control).await;
    resources.finished = true;
    report
}

async fn step<T, E, F, C>(
    future: F,
    phase: &'static str,
    deadline: tokio::time::Instant,
    supervisor: &mut TaskSupervisor,
    gate: &mut Gate<'_>,
    control: &mut C,
) -> Option<T>
where
    F: Future<Output = Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
    C: AsyncFnMut() -> ControlResult,
{
    tracing::info!(event = "startup_stage", phase, "initializing component");
    tokio::pin!(future);
    loop {
        let active = !supervisor.is_empty();
        tokio::select! {
            biased;
            exit = supervisor.next_exit(), if active => { gate.stop(exit.map_or(StopCause::EmptyTaskSet, StopCause::Task)); return None; }
            _ = sleep_until(deadline) => { gate.stop(StopCause::StartupTimeout(phase)); return None; }
            event = control(), if gate.events_open => if gate.event(event) { return None; },
            result = &mut future => match result {
                Ok(value) => return Some(value),
                Err(source) => { gate.stop(StopCause::Startup { phase, source: Box::new(source) }); return None; }
            },
        }
    }
}

async fn confirmed<T, C>(
    receiver: &mut oneshot::Receiver<T>,
    phase: &'static str,
    deadline: tokio::time::Instant,
    supervisor: &mut TaskSupervisor,
    gate: &mut Gate<'_>,
    control: &mut C,
) -> Option<T>
where
    C: AsyncFnMut() -> ControlResult,
{
    let mut closed = false;
    loop {
        // Dropping an ack sender can wake its receiver just before the JoinSet
        // completion is visible. After ack closure, await the actual task exit
        // under the SAME startup deadline, preserving a bind failure's source.
        tokio::select! {
            biased;
            exit = supervisor.next_exit() => { gate.stop(exit.map_or(StopCause::EmptyTaskSet, StopCause::Task)); return None; }
            _ = sleep_until(deadline) => { gate.stop(StopCause::StartupTimeout(phase)); return None; }
            event = control(), if gate.events_open => if gate.event(event) { return None; },
            result = &mut *receiver, if !closed => match result { Ok(value) => return Some(value), Err(_) => closed = true },
        }
    }
}

async fn start<C>(
    resources: &mut Resources<'_>,
    reader: LifecycleHandle,
    config: &BootConfig,
    build: BuildInfo,
    deadline: tokio::time::Instant,
    control: &mut C,
    executors: &Executors,
) -> Option<SocketAddr>
where
    C: AsyncFnMut() -> ControlResult,
{
    // Resolve before I/O side effects as a defensive check for programmatic
    // callers. Configuration validation already rejected unknown bindings.
    let ticker_target = match executors.resolve(&config.ticker_runtime) {
        Ok(target) => target,
        Err(error) => {
            resources.gate.stop(StopCause::Startup {
                phase: "resolve_ticker_runtime",
                source: Box::new(error),
            });
            return None;
        }
    };
    let http_target = match executors.resolve(&config.http_runtime) {
        Ok(target) => target,
        Err(error) => {
            resources.gate.stop(StopCause::Startup {
                phase: "resolve_http_runtime",
                source: Box::new(error),
            });
            return None;
        }
    };
    let parent = config
        .storage
        .path()
        .parent()
        .expect("validated absolute database path");
    step(
        tokio::fs::create_dir_all(parent),
        "data_directory",
        deadline,
        &mut resources.supervisor,
        &mut resources.gate,
        control,
    )
    .await?;
    resources.storage = Some(StorageOwner::prepare(config.storage.clone()));
    let storage = step(
        resources
            .storage
            .as_ref()
            .expect("owner recorded")
            .initialize(),
        "storage",
        deadline,
        &mut resources.supervisor,
        &mut resources.gate,
        control,
    )
    .await?;
    let hot_reader = resources.reload.reader();
    let (started_tx, mut started_rx) = oneshot::channel();
    let child = resources.gate.root.child_token();
    let worker_reader = reader.clone();
    if let Err(error) = resources.supervisor.spawn_on(
        "ticker",
        ticker_target.handle,
        ticker_target.name,
        async move {
            let thread = std::thread::current();
            tracing::info!(
                event = "task_polled",
                task = "ticker",
                thread = thread.name().unwrap_or("<unnamed>"),
                "task started"
            );
            service_worker::run(hot_reader, worker_reader, child, started_tx)
                .await
                .map_err(|error| Box::new(error) as TaskError)
        },
    ) {
        resources.gate.stop(StopCause::Startup {
            phase: "register_ticker",
            source: Box::new(error),
        });
        return None;
    }
    confirmed(
        &mut started_rx,
        "ticker_confirmation",
        deadline,
        &mut resources.supervisor,
        &mut resources.gate,
        control,
    )
    .await?;
    let (http_tx, mut http_rx) = oneshot::channel();
    let state = service_api::AppState {
        storage,
        lifecycle: reader,
        build,
    };
    let http = config.http.clone();
    let child = resources.gate.root.child_token();
    if let Err(error) =
        resources
            .supervisor
            .spawn_on("http", http_target.handle, http_target.name, async move {
                let thread = std::thread::current();
                tracing::info!(
                    event = "task_polled",
                    task = "http",
                    thread = thread.name().unwrap_or("<unnamed>"),
                    "task started"
                );
                service_api::run(state, http, child, http_tx)
                    .await
                    .map_err(|error| Box::new(error) as TaskError)
            })
    {
        resources.gate.stop(StopCause::Startup {
            phase: "register_http",
            source: Box::new(error),
        });
        return None;
    }
    let address = confirmed(
        &mut http_rx,
        "http_confirmation",
        deadline,
        &mut resources.supervisor,
        &mut resources.gate,
        control,
    )
    .await?;
    loop {
        if let Some(exit) = resources.supervisor.try_next_exit() {
            resources.gate.stop(StopCause::Task(exit));
            return None;
        }
        tokio::select! {
            biased;
            _ = sleep_until(deadline) => { resources.gate.stop(StopCause::StartupTimeout("commit")); return None; }
            event = control(), if resources.gate.events_open => if resources.gate.event(event) { return None; },
            _ = std::future::ready(()) => {
                tracing::info!(tasks = resources.supervisor.len(), "startup confirmed");
                return Some(address);
            }
        }
    }
}

async fn monitor<C>(resources: &mut Resources<'_>, control: &mut C)
where
    C: AsyncFnMut() -> ControlResult,
{
    loop {
        if resources.supervisor.is_empty() {
            resources.reload.stop();
            resources.gate.stop(StopCause::EmptyTaskSet);
            return;
        }
        if let Some(report) = resources.reload.expire_if_due() {
            report.log();
        }
        let busy = resources.reload.is_busy();
        let deadline = resources.reload.deadline();
        tokio::select! {
            biased;
            exit = resources.supervisor.next_exit() => {
                resources.reload.stop();
                resources.gate.stop(exit.map_or(StopCause::EmptyTaskSet, StopCause::Task)); return;
            }
            event = control(), if resources.gate.events_open => match event {
                Ok(Event::Reload) => resources.reload.request(),
                other => { resources.reload.stop(); if resources.gate.event(other) { return; } }
            },
            _ = wait_reload_deadline(deadline) => {},
            completion = resources.reload.next_completion(), if busy => match resources.reload.complete(completion) {
                Ok(report) => report.log(),
                Err(error) => {
                    resources.reload.stop();
                    resources.gate.stop(StopCause::ReloadFailure(error.label())); return;
                }
            },
        }
    }
}

async fn wait_reload_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

// true means complete; false means deadline or a second stop request. Each
// iteration reuses the same deadline, never one full timeout per task.
async fn collect<C>(
    supervisor: &mut TaskSupervisor,
    reload: &mut ReloadController,
    gate: &mut Gate<'_>,
    control: &mut C,
    deadline: tokio::time::Instant,
    exits: &mut Vec<TaskExit>,
    fault: &mut bool,
) -> bool
where
    C: AsyncFnMut() -> ControlResult,
{
    while !supervisor.is_empty() || reload.is_busy() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        let tasks_active = !supervisor.is_empty();
        let reload_active = reload.is_busy();
        tokio::select! {
            biased;
            exit = supervisor.next_exit(), if tasks_active => if let Some(exit) = exit { exits.push(exit); },
            completion = reload.next_completion(), if reload_active => { *fault |= !reload.discard_for_shutdown(completion); },
            _ = sleep_until(deadline) => return false,
            event = control(), if gate.events_open => match event {
                Ok(Event::Stop) => return false,
                Ok(Event::Reload) => tracing::warn!(event = "reload_ignored", "reload ignored during shutdown"),
                Err(_) => { gate.events_open = false; *fault = true; return false; }
            },
        }
    }
    true
}

async fn cleanup<C>(resources: &mut Resources<'_>, control: &mut C) -> RunReport
where
    C: AsyncFnMut() -> ControlResult,
{
    resources.reload.stop();
    resources.supervisor.close_registration();
    let plan = resources
        .gate
        .context
        .plan
        .expect("stop records the shared plan");
    let budget = resources.gate.budget;
    let mut tasks = Vec::new();
    let mut failed = plan.invalid;
    let mut forced = false;
    if !collect(
        &mut resources.supervisor,
        &mut resources.reload,
        &mut resources.gate,
        control,
        stage_deadline(plan.grace, budget.grace),
        &mut tasks,
        &mut failed,
    )
    .await
    {
        forced = true;
        resources.gate.writer.publish(Phase::Forcing);
        tracing::warn!(event = "shutdown_forcing", pending = ?resources.supervisor.pending_names(), "graceful wait ended; requesting abort");
        resources.supervisor.abort_remaining();
        resources.reload.abort_remaining();
        collect(
            &mut resources.supervisor,
            &mut resources.reload,
            &mut resources.gate,
            control,
            stage_deadline(plan.abort_reap, budget.abort_reap),
            &mut tasks,
            &mut failed,
        )
        .await;
    }
    let mut unreaped = resources.supervisor.pending_names();
    if resources.reload.is_busy() {
        unreaped.push(reload::JOB_NAME);
    }
    let first_http_ok = matches!(&resources.gate.context.cause, Some(StopCause::Task(exit)) if exit.name == "http" && exit.kind.returned());
    let http_proven = !resources.committed
        || first_http_ok
        || tasks
            .iter()
            .any(|exit| exit.name == "http" && exit.kind.returned());
    let storage = if let Some(owner) = resources.storage.as_ref() {
        if !unreaped.is_empty() || !http_proven {
            StorageClose::SkippedUnproven
        } else {
            let (outcome, interrupted, source_failed) = close_storage(
                owner.close(),
                stage_deadline(plan.storage_close, budget.storage_close),
                &mut resources.gate,
                control,
            )
            .await;
            forced |= interrupted;
            failed |= source_failed;
            outcome
        }
    } else {
        StorageClose::NotCreated
    };
    tracing::info!(
        event = "storage_close_result",
        outcome = storage.label(),
        "storage cleanup completed"
    );
    failed |= !unreaped.is_empty()
        || tasks.iter().any(|task| !task.kind.returned())
        || !matches!(storage, StorageClose::NotCreated | StorageClose::Closed);
    RunReport {
        cause: resources
            .gate
            .context
            .cause
            .take()
            .expect("stop cause recorded"),
        tasks,
        unreaped,
        forced,
        cleanup_failed: failed,
        storage,
    }
}

async fn close_storage<F, C>(
    future: F,
    deadline: tokio::time::Instant,
    gate: &mut Gate<'_>,
    control: &mut C,
) -> (StorageClose, bool, bool)
where
    F: Future<Output = ()>,
    C: AsyncFnMut() -> ControlResult,
{
    let close = timeout_at(deadline, future);
    tokio::pin!(close);
    let mut source_failed = false;
    loop {
        tokio::select! {
            biased;
            result = &mut close => return (if result.is_ok() { StorageClose::Closed } else { StorageClose::TimedOut }, false, source_failed),
            event = control(), if gate.events_open => match event {
                Ok(Event::Stop) => return (StorageClose::Interrupted, true, source_failed),
                Ok(Event::Reload) => tracing::warn!(event = "reload_ignored", "reload ignored during storage close"),
                Err(_) => { gate.events_open = false; source_failed = true; }
            },
        }
    }
}

#[cfg(test)]
mod tests;
