//! 任务面契约：任务面返回 future，spawn 到哪个 runtime 由装配层决定。
//!
//! 一条纪律：**任务面里不写 spawn**。任务面只描述"要一直做什么"，生命周期（启动、重启、关停）
//! 全部由 supervisor 负责；这样任务可以被测试直接 `await`，也可以在任意 runtime 上跑。

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use {{crate_prefix_snake}}_core::Error;

use crate::id::{RuntimeId, TaskKey};
use crate::shutdown::StopToken;

/// 任务面的返回类型：一个 `Send + 'static` 的 future，产出 `Result<(), Error>`。
pub type TaskFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'static>>;

/// 任务面工厂：每次（重）启动都调用一次，拿到一个新的 [`TaskContext`]。
pub type TaskFactory = Box<dyn Fn(TaskContext) -> TaskFuture + Send + Sync + 'static>;

/// 任务面拿到的东西：身份 + 两级停止观察点。
#[derive(Clone)]
pub struct TaskContext {
    key: TaskKey,
    runtime: RuntimeId,
    stop: StopToken,
}

impl TaskContext {
    pub(crate) fn new(key: TaskKey, runtime: RuntimeId, stop: StopToken) -> Self {
        Self { key, runtime, stop }
    }

    /// 任务名（日志与退出记录用它）。
    pub fn key(&self) -> &TaskKey {
        &self.key
    }

    /// 所在 runtime。
    pub fn runtime(&self) -> RuntimeId {
        self.runtime
    }

    /// `draining` 观察点：该收尾了，但还没被取消。
    pub async fn draining(&self) {
        self.stop.draining().await;
    }

    /// `cancelled` 观察点：做最后清理后尽快返回。
    pub async fn cancelled(&self) {
        self.stop.cancelled().await;
    }

    /// 本化身是否已被单独取消（重启替换）：用于区分"关停"与"被替换"。
    pub fn is_individual_cancelled(&self) -> bool {
        self.stop.is_individual_cancelled()
    }
}

impl fmt::Debug for TaskContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskContext")
            .field("key", &self.key)
            .field("runtime", &self.runtime)
            .finish()
    }
}

/// 退避参数（半热）：配置里的 `[supervisor]` 两个毫秒值写进这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub base: Duration,
    pub cap: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(500),
            cap: Duration::from_secs(30),
        }
    }
}

/// 共享退避参数：装配层在启动时构造、在配置重载（半热段）时写入；
/// supervisor 在**调度下一次重启时**读取——这就是"半热"的落点。
pub type SharedBackoff = Arc<RwLock<Backoff>>;

/// 构造共享退避参数。
pub fn shared_backoff(backoff: Backoff) -> SharedBackoff {
    Arc::new(RwLock::new(backoff))
}

/// 写一次退避参数（配置重载路径用它；可热段没有这个动作）。
pub fn set_shared_backoff(shared: &SharedBackoff, backoff: Backoff) {
    if let Ok(mut guard) = shared.write() {
        *guard = backoff;
    }
}

/// 重启策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPolicy {
    /// 退出是终态（默认）。
    Never,
    /// 自行失败/panic 后重启；`max_restarts` 次连续失败后进入终态。
    /// 退避来自 [`SharedBackoff`]（半热），连续稳定运行满 `stability` 后失败计数清零。
    Restart {
        max_restarts: u32,
        stability: Duration,
    },
}

impl RestartPolicy {
    pub const fn never() -> Self {
        Self::Never
    }

    /// 带默认稳定窗口（60s）的重启策略。
    pub fn restart(max_restarts: u32) -> Self {
        Self::Restart {
            max_restarts,
            stability: Duration::from_secs(60),
        }
    }
}

/// 任务注册项：注册不等于运行；`Supervisor::start` 才是提交点。
pub struct TaskSpec {
    pub(crate) key: TaskKey,
    pub(crate) runtime: RuntimeId,
    pub(crate) fatal: bool,
    pub(crate) restart: RestartPolicy,
    pub(crate) factory: TaskFactory,
}

impl TaskSpec {
    /// 默认：`fatal = true`（自行退出会让进程优雅关停），`restart = Never`。
    pub fn new(
        key: TaskKey,
        runtime: RuntimeId,
        factory: impl Fn(TaskContext) -> TaskFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            key,
            runtime,
            fatal: true,
            restart: RestartPolicy::Never,
            factory: Box::new(factory),
        }
    }

    /// 自行退出是否算进程级失败（默认是）。可选后台任务设 `false`。
    pub fn fatal(mut self, fatal: bool) -> Self {
        self.fatal = fatal;
        self
    }

    pub fn restart(mut self, policy: RestartPolicy) -> Self {
        self.restart = policy;
        self
    }

    pub fn key(&self) -> &TaskKey {
        &self.key
    }

    pub fn runtime(&self) -> RuntimeId {
        self.runtime
    }

    pub fn is_fatal(&self) -> bool {
        self.fatal
    }

    pub fn policy(&self) -> &RestartPolicy {
        &self.restart
    }
}

impl fmt::Debug for TaskSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskSpec")
            .field("key", &self.key)
            .field("runtime", &self.runtime)
            .field("fatal", &self.fatal)
            .field("restart", &self.restart)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let spec = TaskSpec::new(TaskKey::new("x"), RuntimeId::MAIN, |_| {
            Box::pin(async { Ok(()) })
        });
        assert!(spec.is_fatal(), "默认应当 fail-fast");
        assert_eq!(*spec.policy(), RestartPolicy::Never);
    }

    #[test]
    fn shared_backoff_is_shared() {
        let shared = shared_backoff(Backoff::default());
        let clone = shared.clone();
        set_shared_backoff(
            &shared,
            Backoff {
                base: Duration::from_millis(10),
                cap: Duration::from_millis(20),
            },
        );
        assert_eq!(clone.read().expect("读锁").base, Duration::from_millis(10));
    }
}
