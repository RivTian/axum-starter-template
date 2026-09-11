//! 构建元数据、部署路径与时间基元。

mod build_info;
mod paths;
mod time;

pub use build_info::BuildInfo;
pub use paths::{CONFIG_DIR, DATA_DIR, config_file_name, default_config_path, install_root};
pub use time::{TimestampMs, now_ms};
