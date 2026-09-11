//! `{{crate_prefix}}-worker` —— 示例任务面 `ticker`。
//!
//! 它是「一个面级 crate 该长什么样」的参照，把它改成真实的面时保留这几条：
//!
//! - **窄门面**：对 app 只有一个入口 [`ticker`]；内部结构一律不转出；
//! - **面返回 future，app 决定 spawn 到哪个 runtime**：入口不 `tokio::spawn`，
//!   绑定 runtime 是装配层的事（面内部再 `tokio::spawn` 的子任务会自然落在
//!   被 spawn 到的那个 runtime 上）；
//! - **取消是 `select!` 的第一分支**（`biased`），关停优先于新工作；
//! - **可热字段每 tick 现读**：`ConfigHandle::current()`，热重载下一拍生效；
//! - **不初始化 tracing、不落库细节、不依赖 api**；需要存储就加 `storage` 边并在评审说明。

mod ticker;

pub use ticker::run as ticker;
