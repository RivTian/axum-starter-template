//! supervisor：注册、启动、重启与关停编排。
//!
//! 结构：
//! - **账本**：`specs`（注册的任务）、`live`（当前化身，同一 key 至多一条）、`id_map`（spawn 出去的
//!   tokio 任务 Id → key），所有 spawn 都进同一个 `JoinSet`——"不留 detached"就是靠这一点：
//!   supervisor 结束前会把 JoinSet 收干（或 abort 后记清谁没回来）。
//! - **提交点**：`register` 只挂账，`start` 才校验 runtime 并 spawn（之前失败不留半个任务在跑）。
//! - **运行期**：等四类事件（停止信号 / 命令 / 任务结束 / 重启到期），任务结束按五类退出记账，
//!   并按策略决定"重启 / 忽略 / 触发进程级失败"。
//! - **关停**：drain → cancel → 限时收割 → abort + reap → 资源逆序关闭；每一步都有绝对上限，
//!   二次信号只把剩余等待压到 `forced_phase`，不刷新截止。

use std::cmp::min;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use {{crate_prefix_snake}}_core::{Error, ErrorKind};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::{AbortHandle, Id, JoinError, JoinSet};
use tokio::time::{Instant, timeout_at};
use tokio_util::sync::CancellationToken;

use crate::exit::{ExitCause, ExitRecord, TaskOutcome};
use crate::id::{RuntimeSet, TaskKey};
use crate::shutdown::{
    ShutdownBudget, ShutdownReport, StopPhase, StopSignal, StopToken, StopTrigger,
};
use crate::spec::{RestartPolicy, SharedBackoff, TaskContext, TaskFuture, TaskSpec};

/// 资源关闭器：按注册顺序的**逆序**执行（最后打开的最先关；存储因此最后关）。
struct Resource {
    name: String,
    close: TaskFuture,
}

/// 当前化身的账本。
struct Live {
    incarnation: u64,
    abort: AbortHandle,
    cancel: CancellationToken,
    started_at: Instant,
    /// 这次退出是不是"被 supervisor 替换"（决定 `Restarted` 分类）。
    replacing: bool,
}

enum Command {
    Restart {
        key: TaskKey,
        reply: oneshot::Sender<Result<(), Error>>,
    },
}

/// 一次事件循环的产出（把 select 的借用限制在 `next_event` 内部）。
enum Event {
    StopRequested,
    Command(Command),
    Completed(Result<(Id, Result<(), Error>), JoinError>),
    RestartDue,
    Noop,
}

/// 任务监督器。
pub struct Supervisor {
    runtimes: RuntimeSet,
    budget: ShutdownBudget,
    backoff: SharedBackoff,
    signal: StopSignal,
    specs: BTreeMap<TaskKey, Arc<TaskSpec>>,
    pending: Vec<Arc<TaskSpec>>,
    resources: Vec<Resource>,
    joins: JoinSet<Result<(), Error>>,
    id_map: BTreeMap<Id, TaskKey>,
    live: BTreeMap<TaskKey, Live>,
    incarnation: BTreeMap<TaskKey, u64>,
    restarts_done: BTreeMap<TaskKey, u32>,
    restart_at: BTreeMap<TaskKey, Instant>,
    commands_tx: mpsc::UnboundedSender<Command>,
    commands_rx: mpsc::UnboundedReceiver<Command>,
    events: broadcast::Sender<ExitRecord>,
    records: Vec<ExitRecord>,
    fatal_trigger: Option<StopTrigger>,
    stop_requested: bool,
    started: bool,
}

impl Supervisor {
    /// 构造。`backoff` 是半热参数（配置重载会写它），`signal` 由装配层持有用于推进停止相位。
    pub fn new(
        runtimes: RuntimeSet,
        budget: ShutdownBudget,
        backoff: SharedBackoff,
        signal: StopSignal,
    ) -> Self {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        Self {
            runtimes,
            budget,
            backoff,
            signal,
            specs: BTreeMap::new(),
            pending: Vec::new(),
            resources: Vec::new(),
            joins: JoinSet::new(),
            id_map: BTreeMap::new(),
            live: BTreeMap::new(),
            incarnation: BTreeMap::new(),
            restarts_done: BTreeMap::new(),
            restart_at: BTreeMap::new(),
            commands_tx,
            commands_rx,
            events,
            records: Vec::new(),
            fatal_trigger: None,
            stop_requested: false,
            started: false,
        }
    }

    /// 注册任务面（只挂账，不 spawn）。同一 key 重复注册是错误。
    pub fn register(&mut self, spec: TaskSpec) -> Result<(), Error> {
        if self.started {
            return Err(Error::new(
                ErrorKind::Startup,
                format!("supervisor 已经启动，不能再注册任务 `{}`", spec.key()),
            ));
        }
        if self.specs.contains_key(spec.key())
            || self
                .pending
                .iter()
                .any(|pending| pending.key() == spec.key())
        {
            return Err(Error::new(
                ErrorKind::Startup,
                format!(
                    "任务 key 重复注册：`{}`（同一时刻同一 key 只能有一个化身）",
                    spec.key()
                ),
            ));
        }
        self.pending.push(Arc::new(spec));
        Ok(())
    }

    /// 注册一个"最后关闭的资源"（如存储池）。逆注册序关闭。
    pub fn register_resource(
        &mut self,
        name: impl Into<String>,
        close: impl std::future::Future<Output = Result<(), Error>> + Send + 'static,
    ) -> Result<(), Error> {
        let name = name.into();
        if self.resources.iter().any(|resource| resource.name == name) {
            return Err(Error::new(
                ErrorKind::Startup,
                format!("资源 `{name}` 重复注册"),
            ));
        }
        self.resources.push(Resource {
            name,
            close: Box::pin(close),
        });
        Ok(())
    }

    /// 订阅退出记录（装配层用它打日志，测试用它断言五类退出）。
    pub fn subscribe(&self) -> broadcast::Receiver<ExitRecord> {
        self.events.subscribe()
    }

    /// 供任务在运行期发起的操作（当前只有"重启某个 key"）。
    pub fn handle(&self) -> SupervisorHandle {
        SupervisorHandle {
            commands: self.commands_tx.clone(),
        }
    }

    /// **提交点**：校验全部 runtime 可解析，然后 spawn 全部已注册任务。
    ///
    /// 校验失败时不会 spawn 任何任务（不会留下半个进程在跑）。
    pub fn start(&mut self) -> Result<(), Error> {
        if self.started {
            return Err(Error::new(ErrorKind::Startup, "supervisor 已经启动过了"));
        }
        for spec in &self.pending {
            self.runtimes.resolve(spec.runtime)?;
        }
        let pending = std::mem::take(&mut self.pending);
        for spec in pending {
            self.specs.insert(spec.key.clone(), spec.clone());
            self.spawn(&spec)?;
        }
        self.started = true;
        Ok(())
    }

    /// 运行到停止，然后走完整关停序列，返回诚实报告。
    pub async fn run(mut self, signal: &StopSignal) -> ShutdownReport {
        let started = Instant::now();
        let mut phase_rx = signal.subscribe();
        let mut trigger = None;

        while trigger.is_none() {
            match self.next_event(&mut phase_rx).await {
                Event::StopRequested => trigger = Some(StopTrigger::Signal),
                Event::Command(command) => self.handle_command(command).await,
                Event::Completed(joined) => {
                    self.handle_completion(joined);
                }
                Event::RestartDue => self.spawn_due_restarts(),
                Event::Noop => {}
            }
            if let Some(fatal) = self.fatal_trigger.take() {
                trigger = Some(fatal);
            }
        }

        self.shutdown(
            &mut phase_rx,
            trigger.unwrap_or(StopTrigger::Signal),
            started,
        )
        .await
    }

    async fn next_event(&mut self, phase_rx: &mut watch::Receiver<StopPhase>) -> Event {
        let joins_ready = !self.joins.is_empty();
        let restart_at = self.next_restart_at();
        tokio::select! {
            biased;
            _ = phase_rx.wait_for(|phase| *phase >= StopPhase::Draining) => Event::StopRequested,
            command = self.commands_rx.recv() => {
                match command {
                    Some(command) => Event::Command(command),
                    None => Event::Noop,
                }
            }
            joined = self.joins.join_next_with_id(), if joins_ready => {
                Event::Completed(joined.expect("guarded by !joins.is_empty()"))
            }
            _ = sleep_until_opt(restart_at) => Event::RestartDue,
        }
    }

    // ── 运行期 ──────────────────────────────────────────────────────────────────────────

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::Restart { key, reply } => self.restart_key(&key, reply).await,
        }
    }

    /// 替换一个 key 的化身：先收旧（有界），收干净才 spawn 新的——单化身不变量在重启路径上也成立。
    async fn restart_key(&mut self, key: &TaskKey, reply: oneshot::Sender<Result<(), Error>>) {
        let Some(spec) = self.specs.get(key).cloned() else {
            let _ = reply.send(Err(Error::task(format!("没有注册任务 `{key}`"))));
            return;
        };
        match self.live.get(key) {
            None => {
                let _ = reply.send(Err(Error::task(format!("任务 `{key}` 当前没有化身"))));
                return;
            }
            Some(live) if live.replacing => {
                let _ = reply.send(Err(Error::task(format!("任务 `{key}` 正在重启中"))));
                return;
            }
            Some(_) => {}
        }
        if let Some(live) = self.live.get_mut(key) {
            live.replacing = true;
            live.cancel.cancel();
        }

        let deadline = Instant::now() + self.budget.reap;
        loop {
            if !self.live.contains_key(key) {
                break;
            }
            match timeout_at(deadline, self.joins.join_next_with_id()).await {
                Ok(Some(joined)) => {
                    self.handle_completion(joined);
                }
                _ => break,
            }
        }

        if self.live.contains_key(key) {
            // 旧化身不肯走：保持单化身，不 spawn 新的，并复位替换标记。
            if let Some(live) = self.live.get_mut(key) {
                live.replacing = false;
            }
            let _ = reply.send(Err(Error::task(format!(
                "任务 `{key}` 在收尾窗口内没有结束，重启已放弃（仍是单个化身）"
            ))));
            return;
        }

        let _ = reply.send(self.spawn(&spec));
    }

    fn handle_completion(
        &mut self,
        joined: Result<(Id, Result<(), Error>), JoinError>,
    ) -> Option<TaskKey> {
        let (id, outcome) = match joined {
            Ok((id, Ok(()))) => (id, TaskOutcome::Returned),
            Ok((id, Err(err))) => (
                id,
                TaskOutcome::Failed {
                    message: format!("{}: {}", err.kind(), err.message()),
                },
            ),
            Err(err) => {
                let id = err.id();
                let outcome = if err.is_panic() {
                    TaskOutcome::Panicked {
                        message: panic_message(err),
                    }
                } else {
                    TaskOutcome::Aborted
                };
                (id, outcome)
            }
        };
        let key = self.id_map.remove(&id)?;
        let live = self.live.remove(&key)?;
        let spec = self.specs.get(&key).cloned();
        let runtime = spec
            .as_ref()
            .map_or(crate::id::RuntimeId::MAIN, |spec| spec.runtime);
        let uptime = live.started_at.elapsed();
        let failed = matches!(
            outcome,
            TaskOutcome::Failed { .. } | TaskOutcome::Panicked { .. }
        );
        let cause = if live.replacing {
            ExitCause::Restarted
        } else if self.stop_requested {
            ExitCause::Cancelled
        } else {
            match &outcome {
                TaskOutcome::Returned => ExitCause::Completed,
                TaskOutcome::Failed { .. } => ExitCause::Failed,
                TaskOutcome::Panicked { .. } => ExitCause::Panicked,
                TaskOutcome::Aborted => ExitCause::Cancelled,
            }
        };
        let restarts = self.restarts_done.get(&key).copied().unwrap_or(0);
        let record = ExitRecord {
            key: key.clone(),
            runtime,
            incarnation: live.incarnation,
            cause,
            outcome,
            uptime,
            restarts,
        };
        let _ = self.events.send(record.clone());
        self.records.push(record);

        if !self.stop_requested && !live.replacing {
            if let Some(spec) = spec {
                self.decide_after_exit(&spec, key.clone(), uptime, failed);
            }
        }
        Some(key)
    }

    /// 退出之后：重启，还是让它成为终态（必要时触发进程级失败）。
    fn decide_after_exit(
        &mut self,
        spec: &Arc<TaskSpec>,
        key: TaskKey,
        uptime: Duration,
        failed: bool,
    ) {
        match spec.policy() {
            RestartPolicy::Never => {
                if spec.is_fatal() {
                    self.fatal_trigger = Some(StopTrigger::TaskFailure { key });
                }
            }
            RestartPolicy::Restart {
                max_restarts,
                stability,
            } => {
                if uptime >= *stability {
                    self.restarts_done.insert(key.clone(), 0);
                }
                if !failed {
                    // 正常返回不是"失败"：按终态处理（与 Never 一致）。
                    if spec.is_fatal() {
                        self.fatal_trigger = Some(StopTrigger::TaskFailure { key });
                    }
                    return;
                }
                let done = self.restarts_done.get(&key).copied().unwrap_or(0);
                if done >= *max_restarts {
                    if spec.is_fatal() {
                        self.fatal_trigger = Some(StopTrigger::TaskFailure { key });
                    }
                    return;
                }
                let attempt = done + 1;
                self.restarts_done.insert(key.clone(), attempt);
                let delay = self.backoff_delay(attempt);
                self.restart_at.insert(key, Instant::now() + delay);
            }
        }
    }

    fn backoff_delay(&self, attempt: u32) -> Duration {
        let backoff = *self.backoff.read().expect("退避参数锁被毒化");
        let factor = 1u32 << attempt.saturating_sub(1).min(16);
        backoff.base.saturating_mul(factor).min(backoff.cap)
    }

    fn next_restart_at(&self) -> Option<Instant> {
        self.restart_at.values().copied().min()
    }

    fn spawn_due_restarts(&mut self) {
        let now = Instant::now();
        let due: Vec<TaskKey> = self
            .restart_at
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            self.restart_at.remove(&key);
            if let Some(spec) = self.specs.get(&key).cloned() {
                if let Err(err) = self.spawn(&spec) {
                    self.record_spawn_failure(&spec, &err);
                }
            }
        }
    }

    fn spawn(&mut self, spec: &Arc<TaskSpec>) -> Result<(), Error> {
        let handle = self.runtimes.resolve(spec.runtime)?.clone();
        let incarnation = {
            let counter = self.incarnation.entry(spec.key.clone()).or_insert(0);
            *counter += 1;
            *counter
        };
        let cancel = CancellationToken::new();
        let token = StopToken::new(self.signal.subscribe(), cancel.clone());
        let context = TaskContext::new(spec.key.clone(), spec.runtime, token);
        let future = (spec.factory)(context);
        let abort = self.joins.spawn_on(future, &handle);
        let id = abort.id();
        self.id_map.insert(id, spec.key.clone());
        self.live.insert(
            spec.key.clone(),
            Live {
                incarnation,
                abort,
                cancel,
                started_at: Instant::now(),
                replacing: false,
            },
        );
        Ok(())
    }

    /// spawn 失败（理论上只在重启路径上可能）：记一条记录，必要时触发进程级失败。
    fn record_spawn_failure(&mut self, spec: &Arc<TaskSpec>, err: &Error) {
        let record = ExitRecord {
            key: spec.key.clone(),
            runtime: spec.runtime,
            incarnation: self.incarnation.get(&spec.key).copied().unwrap_or(0),
            cause: ExitCause::Failed,
            outcome: TaskOutcome::Failed {
                message: format!("spawn 失败：{}", err.message()),
            },
            uptime: Duration::ZERO,
            restarts: self.restarts_done.get(&spec.key).copied().unwrap_or(0),
        };
        let _ = self.events.send(record.clone());
        self.records.push(record);
        if spec.is_fatal() {
            self.fatal_trigger = Some(StopTrigger::TaskFailure {
                key: spec.key.clone(),
            });
        }
    }

    // ── 关停 ────────────────────────────────────────────────────────────────────────────

    async fn shutdown(
        &mut self,
        phase_rx: &mut watch::Receiver<StopPhase>,
        trigger: StopTrigger,
        started: Instant,
    ) -> ShutdownReport {
        let mut report = ShutdownReport::new(trigger);
        let hard_deadline = started + self.budget.total;

        // 停止之前就已经结束的任务：先把它们的账记掉，别被误算成 Cancelled。
        while let Some(joined) = self.joins.try_join_next_with_id() {
            self.handle_completion(joined);
        }
        self.stop_requested = true;
        report.forced = *phase_rx.borrow() >= StopPhase::Forced;

        // L2a：drain 窗口（相位已经是 Draining，这里只是给任务一个自己收尾的窗口）。
        if !report.forced
            && wait_for_forced_or_timeout(self.budget.drain, hard_deadline, phase_rx).await
        {
            report.forced = true;
        }

        // L2b：协作取消 + 限时收割。
        self.signal.request_cancel();
        let harvest = if report.forced {
            self.budget.forced_phase
        } else {
            self.budget.harvest
        };
        self.harvest(harvest, hard_deadline, &mut report, phase_rx)
            .await;

        // L2c：abort 未收尾者，再给一个观察窗口。
        let stuck: Vec<TaskKey> = self.live.keys().cloned().collect();
        for key in &stuck {
            if let Some(live) = self.live.get(key) {
                live.abort.abort();
            }
            report.aborted.push(key.clone());
        }
        let reap = if report.forced {
            self.budget.forced_phase
        } else {
            self.budget.reap
        };
        let reap_deadline = min(Instant::now() + reap, hard_deadline);
        while !self.joins.is_empty() {
            match timeout_at(reap_deadline, self.joins.join_next_with_id()).await {
                Ok(Some(joined)) => {
                    self.handle_completion(joined);
                }
                _ => break,
            }
        }
        report.still_running = self.live.keys().cloned().collect();

        // L2d：资源逆注册序关闭（存储最后关）。
        let mut pending = std::mem::take(&mut self.resources);
        let resources_budget = if report.forced {
            self.budget.forced_phase
        } else {
            self.budget.resources
        };
        let resources_deadline = min(Instant::now() + resources_budget, hard_deadline);
        while let Some(resource) = pending.pop() {
            if Instant::now() >= resources_deadline {
                report
                    .resource_failures
                    .push(format!("{}: 关闭超时（预算用尽）", resource.name));
                continue;
            }
            match timeout_at(resources_deadline, resource.close).await {
                Ok(Ok(())) => report.resources_closed.push(resource.name),
                Ok(Err(err)) => report.resource_failures.push(format!(
                    "{}: {}: {}",
                    resource.name,
                    err.kind(),
                    err.message()
                )),
                Err(_) => report
                    .resource_failures
                    .push(format!("{}: 关闭超时", resource.name)),
            }
        }

        report.elapsed = started.elapsed();
        report.records = std::mem::take(&mut self.records);
        report
    }

    async fn harvest(
        &mut self,
        budget: Duration,
        hard_deadline: Instant,
        report: &mut ShutdownReport,
        phase_rx: &mut watch::Receiver<StopPhase>,
    ) {
        let deadline = min(Instant::now() + budget, hard_deadline);
        while !self.joins.is_empty() {
            let wait_until = if report.forced {
                min(Instant::now() + self.budget.forced_phase, deadline)
            } else {
                deadline
            };
            tokio::select! {
                biased;
                // 只观察一次：相位已经到 Forced 时 wait_for 会立刻满足，重复轮询会变成空转。
                _ = phase_rx.wait_for(|phase| *phase >= StopPhase::Forced), if !report.forced => {
                    report.forced = true;
                }
                joined = self.joins.join_next_with_id() => {
                    match joined {
                        Some(joined) => {
                            if let Some(key) = self.handle_completion(joined) {
                                report.stopped.push(key);
                            }
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep_until(wait_until) => break,
            }
        }
    }
}

/// 运行期操作入口（可克隆，交给需要重启能力的任务面使用）。
#[derive(Debug, Clone)]
pub struct SupervisorHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl SupervisorHandle {
    /// 请求替换某个 key 的化身：先收旧（有界），收干净才起新的。
    /// 旧化身不肯结束 → 返回错误，且**不会**起第二个化身。
    pub async fn restart(&self, key: &TaskKey) -> Result<(), Error> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Restart {
                key: key.clone(),
                reply,
            })
            .map_err(|_| Error::task("supervisor 已经退出，命令无法送达"))?;
        response
            .await
            .map_err(|_| Error::task("supervisor 没有回应重启请求"))?
    }
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// 等满 `duration`（受 `hard_deadline` 约束）→ `false`；被二次信号打断 → `true`。
async fn wait_for_forced_or_timeout(
    duration: Duration,
    hard_deadline: Instant,
    phase_rx: &mut watch::Receiver<StopPhase>,
) -> bool {
    let deadline = min(Instant::now() + duration, hard_deadline);
    tokio::select! {
        biased;
        _ = phase_rx.wait_for(|phase| *phase >= StopPhase::Forced) => true,
        _ = tokio::time::sleep_until(deadline) => false,
    }
}

fn panic_message(error: JoinError) -> String {
    let payload = error.into_panic();
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<非字符串 panic 载荷>".to_owned()
    }
}
