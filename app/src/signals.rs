//! 监控循环与统一关停

use std::time::Duration;

use {{crate_prefix_snake}}_core::events::AppEvent;
use {{crate_prefix_snake}}_core::task::{TaskExit, TaskSupervisor};
use {{crate_prefix_snake}}_core::{AppError, AppResult};

use crate::state::RuntimeState;

/// 顶层任务的关停宽限期。面内部若有二级收割（reconcile 的 `UnitManager::shutdown`），
/// 其预算要**短于**这个值并留余量：内外两层取同值会让内层吃满、外层没时间收尾
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 运行监控：等待退出信号 / 热重载信号 / 任务异常退出。
pub async fn monitor(state: RuntimeState, mut supervisor: TaskSupervisor) -> AppResult<()> {
    let run_result = run_loop(&state, &mut supervisor).await;
    shutdown_runtime(state, supervisor).await;
    run_result
}

#[cfg(unix)]
async fn run_loop(state: &RuntimeState, supervisor: &mut TaskSupervisor) -> AppResult<()> {
    use tokio::signal::unix::{SignalKind, signal};

    // 尚未登记任何顶层任务：没有监控对象，留痕后正常退出（仍走统一关停序列）。
    // 否则 select 中 next_exit 立即返回 Empty，被 first_failure 判为异常。
    if supervisor.is_empty() {
        tracing::warn!("no runtime tasks registered, exiting");
        return Ok(());
    }

    // 三个订阅者一律在**循环外**建，SIGINT 不例外（别改回循环内的
    // `tokio::signal::ctrl_c()`）：`sighup` 分支体执行期间进程照样在收信号，而
    // `watch::Sender::subscribe()` 会把当前版本标记为已见——循环内新建的订阅者
    // 看不见建立之前的那次广播，落在这个窗口里的 Ctrl-C 就被静默吞掉。而且此时
    // tokio 的 handler 早已接管 SIGINT 的默认终止行为，进程也不会因此退出，
    // 表现就是「按了没反应」。
    //
    // 注册失败在这里 `?` 出去：`monitor` 拿到 Err 仍会跑统一关停序列，
    // 资源清理不会被跳过（见 `monitor`）
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sighup = signal(SignalKind::hangup())?;

    loop {
        tokio::select! {
            _ = sigint.recv() => {
                tracing::info!("shutdown signal received (SIGINT)");
                return Ok(());
            }
            _ = sigterm.recv() => {
                tracing::info!("shutdown signal received (SIGTERM)");
                return Ok(());
            }
            _ = sighup.recv() => {
                // 热重载：失败保留旧配置，绝不半套用（ConfigStore::reload 保证）
                match state.config.reload() {
                    Ok(report) => {
                        state.events.publish(AppEvent::ConfigReloaded {
                            generation: report.generation,
                        });
                        tracing::info!(
                            generation = report.generation,
                            applied = ?report.applied,
                            deferred = ?report.deferred,
                            requires_restart = ?report.requires_restart,
                            "config reloaded"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "config reload rejected, keeping last good");
                    }
                }
            }
            exit = supervisor.next_exit() => {
                return Err(first_failure(exit));
            }
        }
    }
}

/// 非 unix 平台：无 SIGTERM/SIGHUP，仅 ctrl_c。
#[cfg(not(unix))]
async fn run_loop(_state: &RuntimeState, supervisor: &mut TaskSupervisor) -> AppResult<()> {
    if supervisor.is_empty() {
        tracing::warn!("no runtime tasks registered, exiting");
        return Ok(());
    }

    tokio::select! {
        signal = tokio::signal::ctrl_c() => match signal {
            Ok(()) => {
                tracing::info!("shutdown signal received (ctrl_c)");
                Ok(())
            }
            Err(error) => Err(AppError::Io(error)),
        },
        exit = supervisor.next_exit() => Err(first_failure(exit)),
    }
}

/// first-failure 收敛：任一顶层任务退出即结束进程。
fn first_failure(exit: TaskExit) -> AppError {
    tracing::error!(exit = %exit, "top-level task exited, shutting down");
    AppError::Internal(anyhow::Error::new(exit))
}

/// 统一关停序列（所有退出路径共用）：广播 → 取消 → 限时收割 → 关库。
async fn shutdown_runtime(state: RuntimeState, mut supervisor: TaskSupervisor) {
    // ShuttingDown 先于取消广播：给旁路一个在取消风暴前 flush 的机会
    state.events.publish(AppEvent::ShuttingDown);
    state.shutdown.cancel();
    supervisor.wait_for_shutdown(SHUTDOWN_GRACE).await;

    // 存储放到最后关：宽限期内退出的任务可能还在做最后一批写入，
    // 提前关池会把正常落盘变成一堆关连接错误
    state.storage.close().await;
    tracing::info!("storage closed");
}

#[cfg(test)]
mod tests {
    use tokio_util::sync::CancellationToken;

    use {{crate_prefix_snake}}_core::config::ConfigStore;
    use {{crate_prefix_snake}}_core::events::{AppEvent, EventBus};
    use {{crate_prefix_snake}}_core::task::TaskSupervisor;
    use {{crate_prefix_snake}}_core::util::config_file_name;
    use {{crate_prefix_snake}}_testkit::temp_storage;

    use crate::rt::Executors;
    use crate::state::RuntimeState;

    use super::monitor;

    /// 尚未登记任何顶层任务时应正常退出（含统一关停序列），而非被判为 first-failure。
    #[tokio::test]
    async fn empty_supervisor_exits_normally() {
        let (dir, storage) = temp_storage("signals-empty").await;
        let state = RuntimeState {
            config: ConfigStore::load_or_init(dir.join(config_file_name()), dir.clone())
                .expect("init embedded template"),
            events: EventBus::default(),
            storage,
            metrics: Default::default(),
            executors: Executors::from_current(),
            shutdown: CancellationToken::new(),
        };

        let result = monitor(state, TaskSupervisor::new()).await;

        let _ = std::fs::remove_dir_all(&dir);
        result.expect("empty supervisor should exit normally");
    }

    /// SIGTERM 驱动完整关停序列：monitor 正常返回、`ShuttingDown` 事件已
    /// 广播（先于取消，给旁路 flush 的机会）、任务已收割（monitor 返回即证）、
    /// 存储最后关闭。
    ///
    /// 信号是进程级广播：本测试向自身进程发 SIGTERM，risk 面已核对——
    /// 同二进制的其余测试要么不进 select（空 supervisor 早退），要么不
    /// 监听信号；tokio 的 SIGTERM handler 一经注册即接管默认终止行为，
    /// 发信号前的探测就是在等 monitor 完成注册。
    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_drives_the_full_shutdown_sequence() {
        use crate::boot::tests::{http_get_status_line, state_at};
        use {{crate_prefix_snake}}_testkit::test_port;

        let port = test_port(2);
        let state = state_at("sigterm", port).await;
        let mut supervisor = TaskSupervisor::new();
        crate::boot::register_runtime_tasks(&state, &mut supervisor).expect("注册顶层任务");

        // 观测探针先于 move：事件订阅要建立在广播之前
        let mut events_probe = state.events.subscribe();
        let shutdown_probe = state.shutdown.clone();
        let storage_probe = state.storage.clone();

        let monitor_task = tokio::spawn(monitor(state, supervisor));

        // 等 monitor 进入 select（信号 handler 注册完成）；以端口可服务为信标
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let status = http_get_status_line(port, "/v1/service/health").await;
        assert!(status.contains("200"), "关停前端点应在服务: {status}");

        let killed = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("向自身发 SIGTERM");
        assert!(killed.success());

        let result = tokio::time::timeout(std::time::Duration::from_secs(10), monitor_task)
            .await
            .expect("SIGTERM 后 monitor 应在宽限内返回")
            .expect("monitor 任务不该 panic");
        result.expect("SIGTERM 属正常退出路径");

        // 完整序列的证据：事件已广播、根令牌已取消、任务已收割（monitor 返回即证）
        assert_eq!(events_probe.try_recv(), Some(AppEvent::ShuttingDown));
        assert!(shutdown_probe.is_cancelled());
        assert!(
            storage_probe.health().await.is_err(),
            "关停序列的最后一步应关闭存储"
        );
    }
}
