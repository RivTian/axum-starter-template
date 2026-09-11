//! HTTP 服务子配置（`[http]` 段）
//!
//! 全部字段带 serde 默认值：配置文件缺失 `[http]` 段时等价于「全网卡 + 8080」，
//! 零配置可用。
//!
//! # 整段不可热变更（冻结语义）
//!
//! socket 在启动时一次 bind，`TimeoutLayer` 在 `build_router` 时一次成型，运行中
//! 都无法热改。`ConfigStore` 的白名单把 `http.` 前缀整体归入 `requires_restart`，
//! reload 时本段回滚为运行值——watch 里的配置必须始终等于「生效中」的配置，
//! 否则读端看到新端口、实际仍 bind 在旧端口，凡依据配置做判断的代码都在说谎。

use serde::{Deserialize, Serialize};

use super::clamp::clamp_field;

/// `[http]` 段完整配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// 监听地址（默认全网卡）
    pub host: String,
    /// 监听端口
    pub port: u16,
    /// 单个请求的处理上限（毫秒）：`TimeoutLayer` 用它把悬挂请求切断，优雅关停
    /// 的排空才不会被一个不回话的客户端拖满宽限期
    pub request_timeout_ms: u64,
    /// 绑定到哪个附加 runtime（`[runtime.extra.<name>]` 的名字）；缺省主 runtime。
    /// 不可热：绑定在 spawn 那一刻定死。写了未配置的名字，配置加载期就报错
    pub runtime: Option<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 8080,
            request_timeout_ms: 30_000,
            runtime: None,
        }
    }
}

impl HttpConfig {
    /// 软越界钳位。host / port 不钳：空 host、端口冲突都由 bind 在启动期如实报错，
    /// 错误信息比配置层预判更准。
    pub(super) fn sanitize(&mut self) {
        clamp_field(
            &mut self.request_timeout_ms,
            1_000,
            600_000,
            "http",
            "request_timeout_ms",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺段等价默认：零配置可用
    #[test]
    fn empty_toml_yields_defaults() {
        let cfg: HttpConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, HttpConfig::default());
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 8080);
    }

    /// 部分覆盖：改哪项就改哪项，其余保持默认
    #[test]
    fn partial_section_keeps_remaining_defaults() {
        let cfg: HttpConfig = toml::from_str("port = 9090").unwrap();
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.host, "0.0.0.0");
    }

    /// 拼错的键是错误，不是静默忽略
    #[test]
    fn unknown_key_is_rejected() {
        assert!(toml::from_str::<HttpConfig>("prot = 1").is_err());
    }

    #[test]
    fn sanitize_clamps_request_timeout() {
        let mut cfg = HttpConfig {
            request_timeout_ms: 1,
            ..HttpConfig::default()
        };
        cfg.sanitize();
        assert_eq!(cfg.request_timeout_ms, 1_000);
    }
}
