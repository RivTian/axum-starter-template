//! 所有 handler 共享的应用状态：全注入，零全局态
//!
//! 字段全部来自 app 装配层（composition root）——api 自己不构造任何长活
//! 资源。通过 axum `State<AppState>` 提取器进入各 handler。
//!
//! # 字段准入纪律（冻结语义）
//!
//! 仅在出现真实消费者时添加字段，不为将来预留：
//!
//! - `storage`：持久化门面，`/v1/service/ready` 的判据；跨 handler 共享，看不见连接池；
//! - `config`：**读端**（`build_router` 读 `[http].request_timeout_ms`）。配置写端
//!   不在此处——它唯一的合法消费者是将来的 `POST /v1/service/config/reload`，
//!   该端点落地时再加，防止写端被随手拿来改配置；
//! - `metrics`：进程级自省计数（**只读**），`/v1/service/info` 的数据源；
//! - `started_at_ms`：`/v1/service/info` 算 uptime。

use std::sync::Arc;

use {{crate_prefix_snake}}_core::config::ConfigHandle;
use {{crate_prefix_snake}}_core::metrics::Metrics;
use {{crate_prefix_snake}}_core::util::TimestampMs;
use {{crate_prefix_snake}}_storage::Storage;

/// HTTP 面共享状态（Clone 廉价：字段都是句柄 / Arc / Copy）。
#[derive(Clone)]
pub struct AppState {
    /// 持久化门面；以 trait 对象注入，上层不感知 SQLite/PostgreSQL 差异，
    /// 也拿不到连接池——换后端不会波及任何调用方
    pub storage: Arc<dyn Storage>,
    /// 配置读端（廉价 Clone 的 watch 句柄）
    pub config: ConfigHandle,
    /// 进程级自省计数（**接口面只读**）
    pub metrics: Arc<Metrics>,
    /// 进程启动时刻（UTC 毫秒）
    pub started_at_ms: TimestampMs,
}
