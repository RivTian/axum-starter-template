//! 启动：进程边界、装配、提交门。
//!
//! 三个子模块按"离进程有多近"排：[`env`] 贴着操作系统（无分支），`assembly` 只造
//! 对象不起任务，`gate` 决定这一次启动到底算不算成功。

pub(crate) mod assembly;
mod env;
pub(crate) mod gate;

pub use env::ProcessEnv;
