//! 统一错误类型：一个状态码、一个稳定短名、一句话。
//!
//! # 为什么是结构体而不是枚举
//!
//! 这一层有一条硬要求：**5xx 的 message 必须是 `&'static str`**。理由是 5xx 的真实原因
//! 通常来自内部（SQL 报错、panic 负载、序列化失败），原样回给客户端就是一次信息泄漏。
//!
//! 写成枚举的话，这条要求只能靠"每个 5xx 变体都记得别放 `String`"来维持——而"记得"
//! 是会失效的。写成字段全私有的结构体之后，构造途径只有两条：
//!
//! | 构造函数 | 状态码 | message 的类型 |
//! | --- | --- | --- |
//! | [`HttpError::client`] | 由调用方给，`debug_assert` 必须是 4xx | `impl Into<Box<str>>`——可以带细节 |
//! | [`HttpError::server`] | 由调用方给 | `&'static str`——**只能**是字面量 |
//!
//! 于是"一个 `String` 出现在 5xx 的响应体里"这件事**在类型上没有入口**。这正是把一条
//! 纪律换成一条类型事实的样子。
//!
//! # 为什么没有 `From<StorageError> for HttpError`
//!
//! `From` 会被 `?` 隐式调用，于是"这个存储错误该回几"这个决定会散落在每一个
//! 用了 `?` 的 handler 里，谁也看不见它。这里给的是一个**显式的、名字里就写着来源**的
//! [`HttpError::from_storage`]，它内部那个 `match` **没有 `_` 臂**——`StorageError` 加一个
//! 变体，这里当场编译不过，而不是悄悄落进一个 500。
//!
//! # 错误链
//!
//! [`HttpError::from_error_chain`] 沿 `source()` 逐层 downcast。业务错误包了一层
//! （`MyError::Db(StorageError)`）之后，只匹配顶层会把一个本该 404 的东西判成 500——
//! 而 404 和 500 在告警上的待遇完全不同。

use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use service_core::storage::StorageError;

use crate::response;

/// 一次失败的响应。
///
/// 字段全私有：见模块文档。
#[derive(Debug)]
pub struct HttpError {
    status: StatusCode,
    code: &'static str,
    message: Message,
}

/// 响应里那句话。两种来源，区别只在"能不能带细节"。
#[derive(Debug)]
enum Message {
    /// 4xx：细节来自客户端自己送来的东西，回给它不泄漏任何内部信息。
    Detail(Box<str>),
    /// 5xx：只能是字面量。
    Fixed(&'static str),
}

impl Message {
    fn as_str(&self) -> &str {
        match self {
            Self::Detail(text) => text,
            Self::Fixed(text) => text,
        }
    }
}

impl HttpError {
    /// 客户端错误（4xx）。细节会原样出现在响应里。
    ///
    /// # Panics
    ///
    /// debug 构建下，`status` 不是 4xx 时断言失败。用 `debug_assert` 而不是返回
    /// `Result`：调用点全在本 crate 内，传错是编码错误，不是运行期情况。
    #[must_use]
    pub fn client(status: StatusCode, code: &'static str, message: impl Into<Box<str>>) -> Self {
        debug_assert!(
            status.is_client_error(),
            "`client` 只接受 4xx，收到 {status}——5xx 走 `server`，它不收 `String`"
        );
        Self {
            status,
            code,
            message: Message::Detail(message.into()),
        }
    }

    /// 服务端错误（5xx）。`description` 只能是字面量。
    #[must_use]
    pub const fn server(status: StatusCode, code: &'static str, description: &'static str) -> Self {
        Self {
            status,
            code,
            message: Message::Fixed(description),
        }
    }

    /// 路由树上没有这条路径。顶层与 `nest` 内的 fallback 共用它。
    #[must_use]
    pub fn not_found() -> Self {
        Self::client(
            StatusCode::NOT_FOUND,
            "not_found",
            "no route matches this path",
        )
    }

    /// handler 超出了 `http.handler_timeout` 的预算。
    ///
    /// **504，不是 408。** RFC 9110 §15.5.9 把 408 定义成"客户端没有及时把请求发完"——
    /// 责任在客户端。而这里超时的是本服务自己的 handler，客户端什么都没做错。
    /// 这条注释是故意写长的：下一个人看到 504 的第一反应多半是把它"修"回 408。
    #[must_use]
    pub const fn timeout() -> Self {
        Self::server(
            StatusCode::GATEWAY_TIMEOUT,
            "handler_timeout",
            "the handler exceeded its time budget",
        )
    }

    /// 把 axum 提取器的 rejection 转成信封。
    ///
    /// 状态码**直接取 axum 给的那一个**，不在这里重新编一张表。理由有两条：
    ///
    /// 1. 需要区分的五档（400 / 413 / 415 / 422，加上 404）**恰好**就是 axum 0.8.9 的
    ///    默认值。自己抄一份表，只是多一份会与上游漂移的副本。
    /// 2. axum 的分档比一张手抄表细。`FailedToDeserializePathParams` 是手写的（不是宏
    ///    生成的），它把「路径参数类型不符」判成 400，却把「handler 的 `Path<T>` 元数
    ///    与路由上的参数个数对不上」判成 **500**——后者是程序员写错了，不是客户端发错了。
    ///    手抄表几乎必然会把这两种压平成同一个 400。
    ///
    /// 状态码是 5xx 时，`body_text` 会被**丢掉**（它可能含内部细节），换成一句固定的话，
    /// 原文进日志——这是本层「凡是响应里不能说的，日志里必须说」的落点之一。
    #[must_use]
    pub fn from_rejection(status: StatusCode, body_text: String) -> Self {
        let code = code_for(status);
        if status.is_server_error() {
            tracing::error!(
                name: "extractor_rejection_withheld",
                target: "http",
                status = status.as_u16(),
                code,
                detail = %body_text,
                "an extractor rejection was withheld from the response"
            );
            return Self::server(status, code, "the request could not be processed");
        }
        Self::client(status, code, body_text)
    }

    /// 存储门面的错误 → 状态码。
    ///
    /// 这个 `match` **没有 `_` 臂**，七个变体一个不落。它就是"让新增变体编译
    /// 不过"那条纪律的全部实现。
    ///
    /// 回给客户端的话一律是固定字面量，**不带**存储侧的负载：`constraint` 是数据库
    /// 元数据、`context` 是内部说明，它们进日志，不进响应体。
    #[must_use]
    pub fn from_storage(error: &StorageError) -> Self {
        let (status, message) = match error {
            StorageError::Unavailable { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "the storage backend is unavailable",
            ),
            StorageError::Migration { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the storage schema is not in a usable state",
            ),
            StorageError::NotFound => {
                (StatusCode::NOT_FOUND, "the requested entity does not exist")
            }
            StorageError::Conflict => (
                StatusCode::CONFLICT,
                "the request conflicts with the current state",
            ),
            StorageError::UniqueViolation { .. } => {
                (StatusCode::CONFLICT, "a uniqueness constraint was violated")
            }
            StorageError::VersionConflict => (
                StatusCode::CONFLICT,
                "the entity was modified by someone else",
            ),
            StorageError::Internal { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the request could not be processed",
            ),
        };

        // 短名直接取 `core` 的分类名——存储的错误分类只有一份，`api` 不另起一套同义词。
        let code = error.kind_str();

        if status.is_server_error() {
            // 5xx 的响应里那句话是通用的，真正的原因只在这里。
            tracing::error!(
                name: "storage_error_withheld",
                target: "http",
                code,
                %error,
                "a storage error was withheld from the response"
            );
        }

        if status.is_client_error() {
            Self::client(status, code, message)
        } else {
            Self::server(status, code, message)
        }
    }

    /// 沿 `source()` 逐层找 [`StorageError`]，找到就按它分档。
    ///
    /// 业务错误通常是包起来的（`MyError::Db(StorageError)`）。只匹配顶层的话，一个本该
    /// 404 的「不存在」会变成 500，而 404 和 500 在告警上的待遇完全不同——这就是这一路
    /// downcast 存在的理由。
    ///
    /// 找不到就是 500，整条链进日志。
    #[must_use]
    pub fn from_error_chain(error: &(dyn std::error::Error + 'static)) -> Self {
        let mut hop: Option<&(dyn std::error::Error + 'static)> = Some(error);
        while let Some(current) = hop {
            if let Some(storage) = current.downcast_ref::<StorageError>() {
                return Self::from_storage(storage);
            }
            hop = current.source();
        }

        tracing::error!(
            name: "unclassified_error",
            target: "http",
            %error,
            "an error reached the http layer without a classification"
        );
        Self::server(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the request could not be processed",
        )
    }

    /// 将要发出的状态码。
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// 信封里的 `error` 短名。
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message.as_str(), self.code)
    }
}

impl std::error::Error for HttpError {}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        response::envelope(self.status, self.code, self.message.as_str())
    }
}

/// 状态码 → 信封里的稳定短名。
///
/// 这里**有** `_` 臂，与 [`HttpError::from_storage`] 那个刻意没有的不是一回事：
/// `StatusCode` 是第三方的开放集合，穷举它既不可能也没有意义；而 `StorageError` 是
/// 本仓库自己的封闭集合，穷举它正是为了让"加了变体忘了分档"编译不过。
fn code_for(status: StatusCode) -> &'static str {
    // 匹配 `u16` 而不是 `StatusCode` 常量：`StatusCode` 内层是 `NonZeroU16`，能不能出现在
    // 模式位置取决于 structural-match，靠它等于把这段代码押在一个上游实现细节上。
    match status.as_u16() {
        400 => "bad_request",
        404 => "not_found",
        405 => "method_not_allowed",
        413 => "payload_too_large",
        415 => "unsupported_media_type",
        422 => "unprocessable_entity",
        503 => "service_unavailable",
        504 => "handler_timeout",
        _ if status.is_server_error() => "internal",
        _ => "bad_request",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `StorageError` 的七个变体各造一个。没有 `Default`，也不该有——这里逐个写出来
    /// 反而是好事：加了变体，这个数组编译不过之前，`from_storage` 就已经先红了。
    fn every_variant() -> Vec<StorageError> {
        vec![
            StorageError::Unavailable { backend: "sqlite" },
            StorageError::Migration {
                stage: service_core::storage::MigrationStage::Apply,
                hint: "hint",
            },
            StorageError::NotFound,
            StorageError::Conflict,
            StorageError::UniqueViolation {
                constraint: "users_email_key".into(),
            },
            StorageError::VersionConflict,
            StorageError::Internal {
                context: "context".into(),
            },
        ]
    }

    #[test]
    fn every_storage_variant_has_a_status_and_a_stable_code() {
        // 映射从第一天起就要覆盖全部变体。`VersionConflict` 在模板里没有构造点，所以
        // 这条用例证明的是**映射**存在且正确；端到端那一段举不出自动化证据，这里也不
        // 编一个假装验证过它的用例。
        for error in every_variant() {
            let http = HttpError::from_storage(&error);
            assert_eq!(
                http.code(),
                error.kind_str(),
                "短名必须直接来自 core 的分类名"
            );
            assert!(
                http.status().is_client_error() || http.status().is_server_error(),
                "{error} 映射出了一个既不是 4xx 也不是 5xx 的状态码"
            );
        }
    }

    #[test]
    fn storage_statuses_split_client_faults_from_server_faults() {
        let expected = [
            (StorageError::Unavailable { backend: "x" }, 503),
            (StorageError::NotFound, 404),
            (StorageError::Conflict, 409),
            (
                StorageError::UniqueViolation {
                    constraint: "c".into(),
                },
                409,
            ),
            (StorageError::VersionConflict, 409),
            (
                StorageError::Internal {
                    context: "c".into(),
                },
                500,
            ),
        ];
        for (error, status) in expected {
            assert_eq!(
                HttpError::from_storage(&error).status().as_u16(),
                status,
                "{error}"
            );
        }
    }

    #[test]
    fn a_storage_payload_never_reaches_the_response_body() {
        // `constraint` 来自数据库元数据，`context` 来自内部说明。两者都不该出现在响应里。
        let error = StorageError::UniqueViolation {
            constraint: "users_email_key".into(),
        };
        let http = HttpError::from_storage(&error);
        assert!(!http.to_string().contains("users_email_key"));

        let error = StorageError::Internal {
            context: "SELECT * FROM secrets".into(),
        };
        let http = HttpError::from_storage(&error);
        assert!(!http.to_string().contains("SELECT"));
    }

    #[derive(Debug)]
    struct Wrapper(StorageError);

    impl fmt::Display for Wrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("business layer failed")
        }
    }

    impl std::error::Error for Wrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[derive(Debug)]
    struct Outer(Wrapper);

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("request failed")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn a_storage_error_two_hops_down_still_decides_the_status() {
        // 只匹配顶层的写法会在这里给出 500。这条用例钉的就是这件事。
        let error = Outer(Wrapper(StorageError::NotFound));
        assert_eq!(
            HttpError::from_error_chain(&error).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn an_error_chain_without_a_storage_error_is_a_plain_500() {
        let error = fmt::Error;
        let http = HttpError::from_error_chain(&error);
        assert_eq!(http.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(http.code(), "internal");
    }

    #[test]
    fn a_server_side_rejection_loses_its_detail_and_a_client_side_one_keeps_it() {
        // axum 把「handler 的 `Path<T>` 与路由参数个数对不上」判成 500——那是程序员的
        // 错，细节对客户端毫无用处，而且可能带路由内部信息。
        let hidden = HttpError::from_rejection(
            StatusCode::INTERNAL_SERVER_ERROR,
            "No paths parameters found for matched route".to_owned(),
        );
        assert!(!hidden.to_string().contains("matched route"));

        let kept = HttpError::from_rejection(
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing field `name` at line 1 column 2".to_owned(),
        );
        assert!(kept.to_string().contains("missing field `name`"));
        assert_eq!(kept.code(), "unprocessable_entity");
    }

    #[test]
    fn the_timeout_is_a_gateway_timeout_and_says_so_in_its_code() {
        // 有人把它改回 408 时，这条用例是第一道拦截。
        assert_eq!(HttpError::timeout().status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(HttpError::timeout().code(), "handler_timeout");
    }
}
