//! API 黑盒测试共用脚手架
//!
//! 端到端走 oneshot：`build_router` + `tower::ServiceExt::oneshot` 发真
//! HTTP 请求、断言状态码与响应体——真路由、真提取器、真序列化都在被测面
//! 里，且**不 bind 端口**（没有 TIME_WAIT / 端口冲突问题）。handler 模块
//! 是私有的，这也是外部消费端点的唯一方式。
//!
//! 每个集成测试二进制各自编译本模块，未用到的助手在别的二进制里是死代码，
//! 属脚手架常态。
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode};
use tower::ServiceExt as _;

use {{crate_prefix_snake}}_api::{AppState, build_router};
use {{crate_prefix_snake}}_core::config::ConfigStore;
use {{crate_prefix_snake}}_core::metrics::Metrics;
use {{crate_prefix_snake}}_core::util::{config_file_name, now_ms};
use {{crate_prefix_snake}}_testkit::temp_storage;

/// 全注入 AppState：临时目录里的内嵌模板配置 + 一用例一库的 SQLite 门面。
///
/// 目录逐调用独占：并发用例同时 `load_or_init` 同一个文件会读到半截 TOML。
pub async fn test_state(tag: &str) -> (PathBuf, AppState) {
    let (dir, storage) = temp_storage(tag).await;
    let path = dir.join(config_file_name());
    let store = ConfigStore::load_or_init(path, dir.clone()).expect("加载测试配置");
    let state = AppState {
        storage,
        config: store.handle(),
        metrics: Arc::new(Metrics::default()),
        started_at_ms: now_ms(),
    };
    (dir, state)
}

/// 发一个无 body 的请求，返回 (状态码, 响应体 JSON)。
pub async fn get_json(state: AppState, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = build_router(state)
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    split(response).await
}

pub async fn split(response: Response<Body>) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("响应体应是 JSON")
    };
    (status, json)
}
