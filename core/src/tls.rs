//! 进程级 rustls `CryptoProvider` 安装
//!
//! # 为什么需要这个模块
//!
//! rustls 0.23 要求**必须**指定一个密码学 provider（`ring` 或 `aws-lc-rs`）。
//! 依赖表里所有走 rustls 的 crate（sqlx 的 `tls-rustls-ring`，将来的 reqwest
//! `rustls-no-provider`）都不自带 provider——`aws-lc-sys` 需要 cmake 与 C 工具链，
//! 交叉编译时纯属负担，所以由本模块显式安装 **`ring`**（纯 Rust，无 C 依赖）。
//!
//! 代价是：provider 不由 feature 自动注入，**首次用到 TLS 之前必须调用
//! [`ensure_crypto_provider`]**，否则 rustls 会在建连时 panic——那是运行期故障，
//! 不是编译错误。
//!
//! # 为什么用 `Once` 而不是只在 `boot_strap` 里装一次
//!
//! 单元测试、集成测试和各类 CLI 子命令都可能绕过 `boot_strap` 直接建连。
//! 用 [`std::sync::Once`] 保证幂等，任何入口调一次都不会漏装也不会重装。

use std::sync::Once;

static INSTALL: Once = Once::new();

/// 确保进程级默认 `CryptoProvider` 已安装为 `ring`。
///
/// 幂等且线程安全：重复调用只有第一次生效。
///
/// # Panics
///
/// 不会 panic。若进程中已经由别处装过 provider（例如宿主程序把本 crate 当库用），
/// `install_default` 返回 `Err`，这里按「已有可用 provider」处理并只记一条 debug 日志。
pub fn ensure_crypto_provider() {
    INSTALL.call_once(|| {
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            // 已被别处安装。不是错误：我们只关心「装了且可用」，不关心是谁装的。
            tracing::debug!("rustls CryptoProvider already installed by another component");
        } else {
            tracing::debug!("rustls CryptoProvider installed: ring");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_crypto_provider_is_idempotent() {
        // 连调三次不应 panic，且事后一定能取到默认 provider
        ensure_crypto_provider();
        ensure_crypto_provider();
        ensure_crypto_provider();

        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "默认 CryptoProvider 应已就位"
        );
    }
}
