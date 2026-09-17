//! 任务监督：supervisor、任务面契约、五类退出、关停预算与报告。
//!
//! 边界（详见 crate README）：
//! - 不安装 subscriber、不依赖 tracing：退出记录走 `broadcast` 通道，**打日志是装配层的事**；
//!   这样测试不需要全局 subscriber 就能断言五类退出。
//! - 不拥有 runtime：spawn 目标由装配层通过 [`RuntimeSet`] 提供；这里只拿 `Handle` 用。
//! - 任务面返回 future，不写 spawn（见 [`TaskSpec`]）。

mod exit;
mod id;
mod shutdown;
mod spec;
mod supervisor;

pub use exit::ExitCause;
pub use exit::ExitRecord;
pub use exit::TaskOutcome;
pub use id::RuntimeId;
pub use id::RuntimeSet;
pub use id::TaskKey;
pub use shutdown::ShutdownBudget;
pub use shutdown::ShutdownReport;
pub use shutdown::StopPhase;
pub use shutdown::StopSignal;
pub use shutdown::StopToken;
pub use shutdown::StopTrigger;
pub use spec::Backoff;
pub use spec::RestartPolicy;
pub use spec::SharedBackoff;
pub use spec::TaskContext;
pub use spec::TaskFactory;
pub use spec::TaskFuture;
pub use spec::TaskSpec;
pub use spec::set_shared_backoff;
pub use spec::shared_backoff;
pub use supervisor::Supervisor;
pub use supervisor::SupervisorHandle;

// 公共 API 里到处是 core::Error，调用方不该为了接一个错误再去加一条直接依赖。
pub use {{crate_prefix_snake}}_core::Error;
pub use {{crate_prefix_snake}}_core::ErrorKind;
