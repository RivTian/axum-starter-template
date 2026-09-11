//! 应用配置的加载与运行时分发。
//!
//! - [`load`] / [`load_or_init`]：读文件 → `${env.NAME:default}` 占位符展开 → 反序列化
//!   → `resolve_paths` → `validate` → `sanitize` 的统一管线，启动与热重载共用一条；
//! - [`ConfigStore`]：唯一写端（装配层持有），[`ConfigHandle`]：廉价 Clone 的读端；
//! - [`ConfigError`]：教学式错误（说清在哪、怎么修）。
//!
//! [`AppConfig`] 的字段按需补齐：只在出现真实消费者时加入对应配置段，
//! 不预放「将来可能要用」的段。

mod app;
mod cfg_http;
mod cfg_runtime;
mod cfg_storage;
mod cfg_ticker;
mod clamp;
mod env_expand;
mod error;
mod path_util;
mod store;

pub use app::{AppConfig, default_config_template, load, load_or_init};
pub use cfg_http::HttpConfig;
pub use cfg_runtime::{ExtraRuntimeConfig, RuntimeConfig};
pub use cfg_storage::{PostgresConfig, SqliteConfig, StorageBackend, StorageConfig};
pub use cfg_ticker::TickerConfig;
pub use env_expand::expand_env_placeholders;
pub use error::ConfigError;
pub use store::{ConfigHandle, ConfigStore, ReloadReport};
