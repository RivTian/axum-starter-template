//! 装配层：把配置、资源、任务面与 supervisor 接在一起。
//!
//! 顺序（也是"提交点"的边界）：
//! 1. `prepare`：构造 `ConfigState`（发布首份生效值）与共享退避参数——纯内存，不碰 IO；
//! 2. `run` 前半段：开池 → 迁移 → 绑定监听 → 构造 supervisor → 注册任务与资源 closer；
//! 3. `supervisor.start()`：提交点，任务开始运行；
//! 4. 等停止信号 → 关停序列 → 如实报告。
//!
//! 骨架里注册两个任务面：`http`（零路由服务）与 `config-watch`（热重载）。
//! 它们都是真实基础设施，不是示例任务面；用户自己的任务面在 `register` 里加（见根 README 切片一）。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use {{crate_prefix_snake}}_config::{Anchor, Config, EnvSource, Reloader, watch_file};
use {{crate_prefix_snake}}_core::{Error, ErrorKind};
use {{crate_prefix_snake}}_runtime::{
    Backoff, RuntimeId, RuntimeSet, SharedBackoff, ShutdownBudget, ShutdownReport, StopPhase,
    StopSignal, Supervisor, TaskContext, TaskKey, TaskSpec, shared_backoff,
};
use {{crate_prefix_snake}}_storage::Db;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use crate::config_state::ConfigState;
use crate::config_watch;
use crate::settings::Settings;
use crate::telemetry::Telemetry;

/// 装配好的运行单元。
pub struct Assembly {
    settings: Settings,
    env: Arc<dyn EnvSource>,
    telemetry: Arc<Telemetry>,
    backoff: SharedBackoff,
    state: Arc<ConfigState>,
    extra_tasks: Vec<TaskSpec>,
    installed: bool,
}

/// 一次完整运行的产出。
pub struct RunOutcome {
    /// 关停报告（如实区分 stopped / still_running / aborted / 资源失败）。
    pub report: ShutdownReport,
    /// HTTP 实际监听地址（`[http].bind` 用端口 0 时，这里是 OS 分配的端口）。
    pub http_address: SocketAddr,
    /// 关停时的生效配置。
    pub config: Config,
}

impl Assembly {
    /// 构造运行单元：发布首份生效配置，准备共享退避参数。
    pub fn prepare(
        settings: Settings,
        env: Arc<dyn EnvSource>,
        telemetry: Arc<Telemetry>,
    ) -> Result<Self, Error> {
        let backoff = shared_backoff(Backoff {
            base: Duration::from_millis(settings.config.supervisor.restart_backoff_ms),
            cap: Duration::from_millis(settings.config.supervisor.restart_backoff_cap_ms),
        });
        let state = Arc::new(ConfigState::new(
            settings.config.clone(),
            telemetry.clone(),
            backoff.clone(),
        ));
        Ok(Self {
            settings,
            env,
            telemetry,
            backoff,
            state,
            extra_tasks: Vec::new(),
            installed: false,
        })
    }

    /// 注册额外的任务面（README 切片一与集成测试用）。
    ///
    /// 内建任务（`http` / `config-watch`）由 `run` 在提交点之前注册；重名会在 supervisor 里报错。
    pub fn register(&mut self, spec: TaskSpec) -> Result<(), Error> {
        if self.installed {
            return Err(Error::new(
                ErrorKind::Startup,
                "装配已经进入运行期，不能再注册任务",
            ));
        }
        self.extra_tasks.push(spec);
        Ok(())
    }

    /// 当前生效配置（启动时是首份，运行期随重载更新）。
    pub fn config(&self) -> Config {
        self.state.current()
    }

    /// 订阅生效配置：任务面读配置都走这里（watch 里的值永远等于生效值）。
    pub fn config_view(&self) -> watch::Receiver<Config> {
        self.state.subscribe()
    }

    /// 路径锚点（可执行文件所在目录）。
    pub fn anchor(&self) -> &Anchor {
        &self.settings.anchor
    }

    /// 配置文件位置。
    pub fn config_source(&self) -> {{crate_prefix_snake}}_config::ConfigSource {
        self.settings.source.clone()
    }

    /// 运行到停止：资源获取 → 注册 → 提交点 → 等停止 → 关停 → 报告。
    pub async fn run(mut self, runtime: &Runtime, stop: StopSignal) -> Result<RunOutcome, Error> {
        let config = self.state.current();

        // ── 提交点之前：资源获取（失败即拒启动，不留半个进程在跑）────────────────
        let busy_timeout = Duration::from_millis(config.storage.busy_timeout_ms);
        let db = Db::open(config.storage.url.expose(), busy_timeout).await?;
        db.migrate().await?;
        let listener = TcpListener::bind(config.http.bind).await.map_err(|err| {
            Error::with_source(
                ErrorKind::Startup,
                format!("绑定 HTTP 监听地址 `{}` 失败", config.http.bind),
                err,
            )
        })?;
        let http_address = listener
            .local_addr()
            .map_err(|err| Error::with_source(ErrorKind::Startup, "读取 HTTP 监听地址失败", err))?;

        // ── supervisor 与任务注册 ───────────────────────────────────────────────
        let runtimes = RuntimeSet::new(runtime.handle().clone());
        let mut supervisor = Supervisor::new(
            runtimes,
            ShutdownBudget::DEFAULT,
            self.backoff.clone(),
            stop.clone(),
        );
        let mut records = supervisor.subscribe();

        // 资源 closer：存储最后关（它是唯一资源，也是最后注册的）。
        supervisor.register_resource("storage", async move { db.close().await })?;

        // 配置文件监听：回调 → mpsc；watcher 活到 run 结束（drop 即停止监听）。
        let config_dir = parent_dir(&self.settings.source.path)?;
        let config_file = file_name(&self.settings.source.path)?;
        let (inbox_tx, inbox_rx) = mpsc::unbounded_channel::<()>();
        let watcher = watch_file(&config_dir, &config_file, move || {
            let _ = inbox_tx.send(());
        })?;

        // 任务面 1：http（监听器随第一个化身存在；重启会明确失败而不是假装还能服务）。
        let listener_slot = Arc::new(Mutex::new(Some(listener)));
        supervisor.register(TaskSpec::new(
            TaskKey::new("http"),
            RuntimeId::MAIN,
            move |ctx: TaskContext| {
                let listener = listener_slot.lock().ok().and_then(|mut slot| slot.take());
                Box::pin(async move {
                    let Some(listener) = listener else {
                        return Err(Error::new(
                            ErrorKind::Task,
                            "HTTP 任务面被重启，但监听器随第一个化身消耗掉了：请先重新绑定再重启",
                        ));
                    };
                    {{crate_prefix_snake}}_http::serve(listener, {{crate_prefix_snake}}_http::router(), async move {
                        ctx.cancelled().await
                    })
                    .await
                })
            },
        ))?;

        // 任务面 2：config-watch（热重载事务：同一条管线 + 三档副作用 + 发布）。
        let reloader = Reloader::new(
            self.settings.source.clone(),
            self.settings.anchor.clone(),
            self.env.clone(),
            config.clone(),
        );
        let state = self.state.clone();
        let reloader_slot = Arc::new(Mutex::new(Some(reloader)));
        let inbox_slot = Arc::new(Mutex::new(Some(inbox_rx)));
        supervisor.register(TaskSpec::new(
            TaskKey::new("config-watch"),
            RuntimeId::MAIN,
            move |ctx: TaskContext| {
                let reloader = reloader_slot.lock().ok().and_then(|mut slot| slot.take());
                let inbox = inbox_slot.lock().ok().and_then(|mut slot| slot.take());
                let state = state.clone();
                Box::pin(async move {
                    let (Some(reloader), Some(inbox)) = (reloader, inbox) else {
                        return Err(Error::new(
                            ErrorKind::Task,
                            "config-watch 被重启，但监听器与重载器随第一个化身消耗掉了",
                        ));
                    };
                    config_watch::run(ctx, reloader, state, inbox).await
                })
            },
        ))?;

        // 用户任务面（切片一 / 测试）。
        for spec in std::mem::take(&mut self.extra_tasks) {
            supervisor.register(spec)?;
        }

        // ── 事件桥：把结构化退出记录打成日志（装配层是唯一知道日志形态的地方）────
        let telemetry = self.telemetry.clone();
        let bridge = runtime.spawn(async move {
            while let Ok(record) = records.recv().await {
                telemetry.log_exit(&record);
            }
        });

        // ── 提交点 ──────────────────────────────────────────────────────────────
        tracing::info!(address = %http_address, "http listener bound");
        supervisor.start()?;
        self.installed = true;
        tracing::info!("tasks started");

        // 控制面：信号监听（不属于任务面，run 结束前收掉）。
        let signals = spawn_signal_listener(runtime, stop.clone());

        let report = supervisor.run(&stop).await;

        signals.abort();
        let _ = signals.await;
        // 事件桥在 supervisor 的发送端 drop 之后自然结束；给一个上限避免拖住关停。
        let _ = tokio::time::timeout(Duration::from_secs(1), bridge).await;
        drop(watcher);

        Ok(RunOutcome {
            report,
            http_address,
            config: self.state.current(),
        })
    }
}

impl std::fmt::Debug for Assembly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assembly")
            .field("source", &self.settings.source)
            .field("anchor", &self.settings.anchor)
            .finish_non_exhaustive()
    }
}

/// 信号监听：第一次把相位推到 Draining，第二次推到 Forced（只加速）。
fn spawn_signal_listener(runtime: &Runtime, stop: StopSignal) -> tokio::task::JoinHandle<()> {
    runtime.spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::error!(error = %err, "无法监听 SIGTERM，只处理 Ctrl-C");
                    return;
                }
            };
            loop {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(err) = result {
                            tracing::error!(error = %err, "无法监听 Ctrl-C");
                            return;
                        }
                        request_stop(&stop);
                    }
                    _ = terminate.recv() => request_stop(&stop),
                }
            }
        }
        #[cfg(not(unix))]
        {
            loop {
                if let Err(err) = tokio::signal::ctrl_c().await {
                    tracing::error!(error = %err, "无法监听 Ctrl-C");
                    return;
                }
                request_stop(&stop);
            }
        }
    })
}

fn request_stop(stop: &StopSignal) {
    match stop.phase() {
        StopPhase::Running => {
            tracing::info!("stop requested (SIGINT/SIGTERM)");
            stop.request_drain();
        }
        StopPhase::Draining => {
            tracing::warn!(
                "second stop request: accelerating shutdown without extending deadlines"
            );
            stop.force();
        }
        StopPhase::Cancelling | StopPhase::Forced => {}
    }
}

fn parent_dir(path: &Path) -> Result<PathBuf, Error> {
    path.parent().map(Path::to_path_buf).ok_or_else(|| {
        Error::new(
            ErrorKind::Startup,
            format!("配置文件路径没有父目录：`{}`", path.display()),
        )
    })
}

fn file_name(path: &Path) -> Result<String, Error> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Startup,
                format!("配置文件路径不是有效文件名：`{}`", path.display()),
            )
        })
}
