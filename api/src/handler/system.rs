//! 系统组：探活与自省，整组挂在 `/v1/service/` 下。
//!
//! - `GET /v1/service/health`：进程活着即 200（liveness）；
//! - `GET /v1/service/ready`：存储自检通过才 200，否则 503 信封（readiness）；
//! - `GET /v1/service/info`：构建串、启动时刻、各面指标快照——「服务跑的哪个版本、
//!   活了多久、面在不在干活」三个问题一次答完。
//!
//! 探针也在 `/v1` 里：版本面之外再留一片无版本路径，等于给自己开了第二套契约，
//! 而探针路径本来就是编排器配置里的一个字符串，跟着版本走没有额外成本。

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;

use {{crate_prefix_snake}}_core::SERVICE_NAME;
use {{crate_prefix_snake}}_core::metrics::TickerSnapshot;
use {{crate_prefix_snake}}_core::util::{BuildInfo, TimestampMs, now_ms};
use {{crate_prefix_snake}}_storage::StorageHealth;

use crate::error::HttpError;
use crate::response::ApiResponse;
use crate::state::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/service/health", get(health))
        .route("/service/ready", get(ready))
        .route("/service/info", get(info))
}

/// liveness：能应答就是活着，不查任何依赖（依赖坏了也不该被编排器反复重启）。
///
/// 没有业务数据可回，于是走成功信封分支：探针只看状态码，body 是给人排查时看的
async fn health() -> ApiResponse<()> {
    ApiResponse::Ok
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Readiness {
    storage: StorageHealth,
}

/// readiness：只看存储自检（任务注册完成之后 http 才起，能应答本身就证明了后者）。
/// 失败回 503 信封而不是 500：这是「暂时别把流量给我」，不是「我坏了」
async fn ready(State(state): State<AppState>) -> Result<ApiResponse<Readiness>, HttpError> {
    match state.storage.health().await {
        Ok(storage) => Ok(ApiResponse::Data(Readiness { storage })),
        Err(error) => {
            tracing::warn!(error = %error, "readiness check failed: storage unhealthy");
            Err(HttpError::ServiceUnavailable("storage unavailable".into()))
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    /// `<name>-<version>(<build> <sha>)`，与启动日志第一条相同
    service: String,
    version: &'static str,
    build: &'static str,
    commit_sha: &'static str,
    description: &'static str,
    started_at_ms: TimestampMs,
    uptime_ms: i64,
    metrics: MetricsView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetricsView {
    ticker: TickerSnapshot,
}

async fn info(State(state): State<AppState>) -> ApiResponse<SystemInfo> {
    let build = BuildInfo::current();
    ApiResponse::Data(SystemInfo {
        service: build.service_description(SERVICE_NAME),
        version: build.version,
        build: build.build,
        commit_sha: build.commit_sha,
        description: build.description,
        started_at_ms: state.started_at_ms,
        uptime_ms: now_ms().saturating_sub(state.started_at_ms),
        metrics: MetricsView {
            ticker: state.metrics.ticker.snapshot(),
        },
    })
}
