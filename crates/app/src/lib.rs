//! 装配层：CLI/配置入口、资源获取、任务注册、信号与关停编排、退出码。
//!
//! 这里是唯一允许"知道全部 crate"的地方；二进制入口 `main.rs` 保持薄，装配逻辑放在 lib 里，
//! 集成测试可以直接构造多套独立装配（零全局态：不需要任何 business static）。

mod assembly;
mod bootstrap;
mod config_state;
mod config_watch;
mod settings;
mod telemetry;

pub use assembly::Assembly;
pub use assembly::RunOutcome;
pub use bootstrap::MAIN_RUNTIME_SHUTDOWN_BUDGET;
pub use bootstrap::ProcessExit;
pub use bootstrap::WATCHDOG_BUDGET;
pub use bootstrap::build_string;
pub use bootstrap::exit_code;
pub use bootstrap::run;
pub use bootstrap::watchdog;
pub use config_state::ConfigState;
pub use settings::Args;
pub use settings::Settings;
pub use settings::process_env;
pub use settings::resolve;
pub use settings::resolve_with_anchor;
pub use telemetry::Telemetry;
