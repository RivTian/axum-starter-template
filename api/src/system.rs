//! 系统端点。三条，各自回答一个**不同**的问题。
//!
//! | 路径 | 问的是 | 查存储 | 受阶段影响 |
//! | --- | --- | --- | --- |
//! | `GET /healthz` | 这个进程还活着吗 | 否 | **否** |
//! | `GET /readyz` | 现在能不能把流量给它 | 是 | 是 |
//! | `GET /v1/info` | 这是哪一版、跑了多久 | 否 | 否 |
//!
//! # `/healthz` 为什么不查存储、也不看阶段
//!
//! 因为编排器拿它做的事是**重启**。存储挂了而进程完好时，重启这个进程没有任何帮助——
//! 它只会把一次"数据库不可用"放大成"数据库不可用 + 全部副本在滚动重启"。
//!
//! 排空期同样不能让它变红：正在优雅关停的进程是**健康**的，它只是不再接新流量。
//! 这两件事分别由 `/readyz` 和存活探针负责，把它们压进一个端点就等于让编排器分不清
//! "杀掉它"和"别再给它流量"。
//!
//! # `/readyz` 为什么要在存储探测的**前后各读一次阶段**
//!
//! 规矩是：跨越 draining 的探测一律不 ready。存储探测是要 `await` 的，而 `SIGTERM`
//! 完全可能正好落在这段 `await` 里。只在探测前读一次的话，会得到这样一次交互：
//!
//! ```text
//! t0  阶段 = Running        读一次 → 可以继续
//! t1  存储探测开始
//! t2  收到 SIGTERM，阶段 → Draining，连接开始排空
//! t3  存储探测返回 Ok
//! t4  回 200 ready          ← 编排器据此又送来一批流量，而这个进程正在关门
//! ```
//!
//! 探测后再读一次就把这个窗口关上了。代价是两次 `borrow()`，各自是一次原子读。
//!
//! 反过来的顺序（先探存储再看阶段）不行：阶段已经不对时就该立刻返回，没必要再去打扰
//! 一个可能很慢的存储。
//!
//! # 原因短名从哪来
//!
//! 阶段那一档直接取 [`Phase::as_str`]——生命周期的名字只有一份，这里不另起一套同义词。
//! 真正需要区分的三档是 `starting` / `draining` / `storage_unavailable`，前两个正是
//! 阶段名；用 `as_str()` 相当于把这张表扩成了全部六个阶段，少一处要手工维护的枚举。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use service_core::lifecycle::Phase;

use crate::error::HttpError;
use crate::response;
use crate::state::AppState;

/// `/readyz` 没通过时的短名。阶段那几档用 [`Phase::as_str`]，只有这一个是本层自己的。
const STORAGE_UNAVAILABLE: &str = "storage_unavailable";

/// 三个响应体刻意都不含 `error` / `message` / `status` 这三个信封键名，
/// 于是「成功响应里三个键一个都不在」那条断言测的是真事。
#[derive(Debug, Serialize)]
struct Live {
    live: bool,
}

#[derive(Debug, Serialize)]
struct Ready {
    ready: bool,
}

#[derive(Debug, Serialize)]
struct Info {
    /// 服务名，取自 `core::BuildInfo`——一路来自 `main.rs` 的 `env!("CARGO_BIN_NAME")`。
    service: &'static str,
    /// 取自 `core::BuildInfo`，也就是 `CARGO_PKG_VERSION`。
    /// **不**在这一层另写一个字符串常量——那么写迟早要错：改了 `Cargo.toml` 的版本号，
    /// 端点还报旧值，而两处都"看起来是对的"。
    version: &'static str,
    uptime_seconds: u64,
}

/// `GET /healthz`
pub(crate) async fn healthz() -> Response {
    response::json(StatusCode::OK, &Live { live: true })
}

/// `GET /readyz`
pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    if let Some(reason) = not_serving(state.lifecycle.current()) {
        return not_ready(reason);
    }

    if let Err(error) = state.storage.health().await {
        // 响应里只说 `storage_unavailable`，是哪一类失败只在日志里。
        tracing::warn!(
            name: "readiness_probe_failed",
            target: "http",
            kind = error.kind_str(),
            %error,
            "the storage readiness probe failed"
        );
        return not_ready(STORAGE_UNAVAILABLE);
    }

    // 探测期间相变了吗？见模块文档里的时序。
    if let Some(reason) = not_serving(state.lifecycle.current()) {
        return not_ready(reason);
    }

    response::json(StatusCode::OK, &Ready { ready: true })
}

/// `GET /v1/info`
pub(crate) async fn info(State(state): State<AppState>) -> Response {
    response::json(
        StatusCode::OK,
        &Info {
            service: state.build.service,
            version: state.build.version,
            uptime_seconds: state.uptime().as_secs(),
        },
    )
}

/// 阶段不对时给出原因短名。
///
/// 判据是 [`Phase::serves_traffic`]——它在 `core` 里是一个**没有 `_` 臂**的 `match`，
/// 于是"加了一个阶段忘了说它算不算在服务"编译不过。这里不重复那张表。
fn not_serving(phase: Phase) -> Option<&'static str> {
    if phase.serves_traffic() {
        None
    } else {
        Some(phase.as_str())
    }
}

fn not_ready(reason: &'static str) -> Response {
    HttpError::server(
        StatusCode::SERVICE_UNAVAILABLE,
        reason,
        "the service is not ready to serve traffic",
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_running_phase_is_serving() {
        // 这条用例守的是"新增阶段默认不算在服务"。`core` 那边加一个变体时，
        // `serves_traffic` 会先红；万一有人给它加了一条 `_ => true`，这里会红。
        for phase in [
            Phase::Starting,
            Phase::Draining,
            Phase::Forcing,
            Phase::Aborting,
            Phase::Stopped,
        ] {
            assert_eq!(not_serving(phase), Some(phase.as_str()));
        }
        assert_eq!(not_serving(Phase::Running), None);
    }

    #[test]
    fn the_readiness_reasons_never_collide_with_a_phase_name() {
        // `storage_unavailable` 必须与任何一个阶段名都不同，否则客户端无法区分
        // "存储探测失败"和"处在某个阶段"。
        for phase in [
            Phase::Starting,
            Phase::Running,
            Phase::Draining,
            Phase::Forcing,
            Phase::Aborting,
            Phase::Stopped,
        ] {
            assert_ne!(phase.as_str(), STORAGE_UNAVAILABLE);
        }
    }
}
