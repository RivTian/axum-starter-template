//! `{{crate_prefix}}-core` —— workspace 的叶子内核。
//!
//! 承载所有 crate 共享、且**不含常驻任务**的内容：配置（[`config`]）、错误基座
//! （[`error`]）、事件总线（[`events`]）、注入式指标（[`metrics`]）、顶层任务监管
//! （[`task`]）、rustls provider 安装（[`tls`]）与构建元数据 / 时间基元（[`util`]）。
//!
//! # 纪律
//!
//! - 不依赖任何兄弟 crate（叶子）；
//! - 不初始化日志：只 `use tracing`，subscriber 安装仅在 `{{crate_prefix}}-app`；
//! - 无 `tokio::spawn` 的常驻任务；无 SQL。
//!
//! # 窄门面
//!
//! 本文件只做 `mod` 声明与显式 `pub use`；子模块各自决定导出什么，调用方走两级路径
//! （`config::ConfigHandle`），不做扁平化 re-export。唯一例外是错误基座：
//! `use {{crate_prefix_snake}}_core::{AppError, AppResult}` 是全 workspace 的通用写法。

pub mod config;
pub mod error;
pub mod events;
pub mod metrics;
pub mod task;
pub mod tls;
pub mod util;

// 窄门面例外：错误基座沿用顶层导出口径。
pub use error::{AppError, AppResult};

/// workspace 统一版本（来自 `workspace.package.version`）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 服务名：启动日志的构建串与自省端点报的 `service` 字段共用同一个值。
///
/// 它等于二进制名。放在 core 而不是各自写一遍字面量：app、api、testkit 报的必须是
/// 同一个串，几处字面量迟早有一处被改漏。
///
/// 另一半理由只对**模板**成立：项目名一旦进到表达式中间，那一行的宽度就随名字长短
/// 在 rustfmt 的 100 列上下翻转，同一份模板便只对部分名字 fmt-clean。绑成常量之后
/// 调用点的宽度与名字无关。门禁见 `scripts/check-fmt-portability.py`。
pub const SERVICE_NAME: &str = "{{crate_name}}";
