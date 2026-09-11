//! 系统组黑盒：探活、自省、`/v1` 兜底。

mod common;

use axum::http::StatusCode;

use {{crate_prefix_snake}}_core::SERVICE_NAME;

use common::{get_json, test_state};

/// liveness 不碰依赖，且走「成功无数据」那条契约：信封，不是裸 JSON
#[tokio::test]
async fn health_is_200_without_touching_dependencies() {
    let (dir, state) = test_state("health").await;
    let (status, json) = get_json(state, "/v1/service/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "success");
    assert_eq!(json["code"], 200);
    let _ = std::fs::remove_dir_all(&dir);
}

/// readiness 的判据是存储自检：开着 200 带健康快照，关了 503 信封
#[tokio::test]
async fn ready_follows_storage_health() {
    let (dir, state) = test_state("ready").await;
    let (status, json) = get_json(state.clone(), "/v1/service/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["storage"]["backend"], "sqlite");
    assert_eq!(json["storage"]["healthy"], true);
    // 成功带数据 = 裸 JSON：信封的键一个都不该在
    assert!(!json.as_object().unwrap().contains_key("status"), "{json}");

    state.storage.close().await;
    let (status, json) = get_json(state, "/v1/service/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["status"], "error");
    assert_eq!(json["code"], 503);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn service_info_reports_build_uptime_and_metrics() {
    let (dir, state) = test_state("info").await;
    state.metrics.ticker.ticked(1);
    let (status, json) = get_json(state, "/v1/service/info").await;
    assert_eq!(status, StatusCode::OK);
    let prefix = format!("{SERVICE_NAME}-");
    assert!(
        json["service"].as_str().unwrap().starts_with(&prefix),
        "{json}"
    );
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    assert!(json["uptimeMs"].as_i64().unwrap() >= 0);
    assert_eq!(json["metrics"]["ticker"]["ticks"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `/v1`、`/v1/`、`/v1/<未知路径>` 三种形态都是 JSON 信封 404
#[tokio::test]
async fn unknown_v1_path_is_a_json_envelope_404() {
    let (dir, state) = test_state("v1_404").await;
    for uri in ["/v1", "/v1/", "/v1/nope"] {
        let (status, json) = get_json(state.clone(), uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(json["status"], "error", "{uri}");
        assert_eq!(json["code"], 404, "{uri}");
        assert!(
            json["description"].as_str().unwrap().contains("GET"),
            "{uri}: {json}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
