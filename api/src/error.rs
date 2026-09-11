//! HTTP 展示层错误类型
//!
//! [`HttpError`] 是 axum handler 的统一 `Err` 类型，负责：
//!
//! 1. 持有 HTTP 语义（状态码对应关系集中在此）；
//! 2. 通过 `From<AppError>` 承接领域层错误（存储语义按错误链还原，其余升格为
//!    [`HttpError::Internal`]）；
//! 3. 实现 [`IntoResponse`]，输出 [`GenericResponse::error`] 那个错误信封。
//!
//! 信封结构体借自 [`crate::response`]，不在这里另起一个：成功信封与错误信封同形是
//! 一条对外契约，两处各写一个结构体就是给它们留了长歪的空间。
//!
//! 变体随消费者进场：404（`/v1` 兜底）、409 与 404 的存储语义映射（[`From<StorageError>`]）、
//! 503（`/v1/service/ready`）、500（`?` 升格）、[`HttpError::Rejected`]（[`crate::extract`]
//! 的提取器拒绝）。每个变体的状态码写在这一处。
//!
//! 存储语义错误的映射集中在 [`storage_semantics`]：`NotFound` → 404；
//! `VersionConflict` / `UniqueViolation` / `Conflict` → 409；其余兜底 500。
//! 仓储进场后 handler 里直接 `?`，不必各自翻译。
//!
//! # 为什么按**错误链**找存储语义，而不是只看顶层类型
//!
//! 只 match 顶层类型的写法只在 handler 直接 `?` 一个 `StorageResult` 时成立。
//! 真实调用栈里仓储调用往往隔着一层领域函数，而领域函数返回 `AppResult`：
//! `?` 会先把 `StorageError` 升格成 `AppError::Storage(anyhow)`，顶层类型就
//! 变成了 `AppError`。此时只看顶层，404 与 409 会**静默退化成 500**——状态码
//! 错了却没有任何报错，是最难发现的一类缺陷。
//!
//! 所以 `From<AppError>` 沿错误链逐层 `downcast_ref` 找存储根因。逐层而不是只看
//! 一层，是因为领域层可能用 `anyhow::Context` 加过说明，那会在链上多垫一个节点。
//! 链上找不到存储语义才落到 500。
//!
//! # 起点为什么不是 [`Error::source`]
//!
//! `AppError::Internal` 是 `#[error(transparent)]`，thiserror 为这类变体生成的
//! `source()` 是「转发给载荷**自己的** source」，载荷本身被跳过去。而领域函数
//! 返回 `anyhow::Result` 时，`?` 落进的正是这个变体：裸 `StorageError` 在
//! `source()` 那条路上根本看不见（链是空的），加过一句 `Context` 的反而看得见
//! ——状态码会取决于调用方有没有顺手写上下文，正是本节要防的那种静默退化。
//!
//! 两个 anyhow 变体因此都从**载荷自身**起走（`storage_root_cause`），而不是从
//! `AppError::source()` 起走。
//!
//! [`Error::source`]: std::error::Error::source
//! # 内部错误的日志与文案分离
//!
//! `Internal` 对外只回泛化文案：错误原文可能带路径、连接串这类不该出网的信息；
//! 原文在这里记一条 `tracing::error!`——集中在错误映射处记，handler 里不用各记各的。
//! 报 5xx 的 `Rejected` 走同一条路：那类拒绝说的是路由与 handler 签名对不上，
//! 是本服务的 bug，文案对调用方既无用又多余。

use std::error::Error as StdError;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

use {{crate_prefix_snake}}_core::AppError;
use {{crate_prefix_snake}}_storage::StorageError;

use crate::response::GenericResponse;

/// HTTP 展示层错误。
#[derive(Debug, Error)]
pub enum HttpError {
    /// 404：目标不存在
    #[error("not found: {0}")]
    NotFound(String),
    /// 409：唯一约束 / 乐观锁 / 领域规则冲突
    #[error("conflict: {0}")]
    Conflict(String),
    /// 503：依赖未就绪（`/v1/service/ready` 的失败分支）
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    /// 提取器拒绝：状态码由 axum 按 RFC 判好（400 / 413 / 415 / 422，偶有 5xx），
    /// 文案是它本来要写进 body 的那段字。唯一构造点是 [`crate::extract`]。
    #[error("{1}")]
    Rejected(StatusCode, String),
    /// 500：领域层错误的兜底承接口。
    ///
    /// 不用 `#[from]`：`From<AppError>` 要先沿错误链找存储语义（见模块文档），
    /// derive 出来的那个会无条件落到 500。
    #[error(transparent)]
    Internal(AppError),
}

impl HttpError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Rejected(status, _) => *status,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// 存储语义 → HTTP 语义。非语义变体（初始化 / 驱动 / 迁移失败）回 `None`：
/// 它们没有对外的 HTTP 含义，由调用方落到 500。
///
/// 取 `&StorageError` 而不是按值：沿错误链找到的根因只能借用（`downcast_ref`），
/// 而两个入口（直接 `?` 与经 `AppError` 升格）要走同一张映射表。
fn storage_semantics(error: &StorageError) -> Option<HttpError> {
    match error {
        StorageError::NotFound(msg) => Some(HttpError::NotFound(msg.clone())),
        StorageError::VersionConflict { expected, actual } => Some(HttpError::Conflict(format!(
            "version conflict: expected {expected}, actual {actual}"
        ))),
        StorageError::UniqueViolation(msg) | StorageError::Conflict(msg) => {
            Some(HttpError::Conflict(msg.clone()))
        }
        StorageError::Init(_) | StorageError::Db(_) | StorageError::Migrate(_) => None,
    }
}

impl From<StorageError> for HttpError {
    fn from(error: StorageError) -> Self {
        storage_semantics(&error).unwrap_or_else(|| Self::Internal(error.into()))
    }
}

/// 沿错误链找存储根因；`None` 表示链上没有存储语义，由调用方落到 500。
///
/// 起点分两种，**不能统一用 `error.source()`**（理由见模块文档「起点为什么不是
/// `Error::source`」）：两个 anyhow 变体从载荷自身起走，其余变体才从 `source()`
/// 起走——`AppError` 自己不可能是 `StorageError`，用它当起点只是白跑一轮。
fn storage_root_cause(error: &AppError) -> Option<HttpError> {
    let mut cursor: Option<&(dyn StdError + 'static)> = match error {
        AppError::Storage(inner) | AppError::Internal(inner) => Some(&**inner),
        other => other.source(),
    };
    while let Some(cause) = cursor {
        if let Some(semantic) = cause
            .downcast_ref::<StorageError>()
            .and_then(storage_semantics)
        {
            return Some(semantic);
        }
        cursor = cause.source();
    }
    None
}

impl From<AppError> for HttpError {
    fn from(error: AppError) -> Self {
        storage_root_cause(&error).unwrap_or_else(|| Self::Internal(error))
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let description = match &self {
            Self::NotFound(msg) | Self::Conflict(msg) | Self::ServiceUnavailable(msg) => {
                msg.clone()
            }
            // 提取器报 5xx = 路由与 handler 签名对不上，是本服务的 bug，与 Internal 同办
            Self::Rejected(_, text) if status.is_server_error() => {
                tracing::error!(rejection = %text, "extractor rejected with a server error");
                "internal error".to_owned()
            }
            Self::Rejected(_, text) => text.clone(),
            Self::Internal(error) => {
                tracing::error!(error = %error, "request failed with internal error");
                "internal error".to_owned()
            }
        };
        // `code` 取自同一个 `status`，信封里的码和 HTTP 状态码不会对不上
        let envelope = GenericResponse::error(status.as_u16(), description);
        (status, Json(envelope)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// 404 信封：status / code / description 三键，code 与状态码一致
    #[tokio::test]
    async fn not_found_maps_to_404_envelope() {
        let response = HttpError::NotFound("no route for GET /v1/x".into()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let json = body_json(response).await;
        assert_eq!(json["status"], "error");
        assert_eq!(json["code"], 404);
        assert_eq!(json["description"], "no route for GET /v1/x");
    }

    /// 存储语义映射：404 / 409 / 500 各归其位
    #[tokio::test]
    async fn storage_errors_map_to_http_semantics() {
        let cases = [
            (StorageError::NotFound("x".into()), StatusCode::NOT_FOUND),
            (
                StorageError::UniqueViolation("u".into()),
                StatusCode::CONFLICT,
            ),
            (StorageError::Conflict("c".into()), StatusCode::CONFLICT),
            (
                StorageError::VersionConflict {
                    expected: 2,
                    actual: 1,
                },
                StatusCode::CONFLICT,
            ),
            (
                StorageError::Init("i".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (error, expected) in cases {
            let http: HttpError = error.into();
            assert_eq!(http.into_response().status(), expected);
        }
    }

    /// 内部错误：500 且文案泛化，原文不出网
    #[tokio::test]
    async fn internal_error_hides_details() {
        let error: HttpError = AppError::Io(std::io::Error::other("secret /var/lib/x")).into();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = body_json(response).await;
        assert_eq!(json["code"], 500);
        assert_eq!(json["description"], "internal error");
    }

    /// **本组是防退化的关键**：领域函数返回 `AppResult` 时 `?` 会把存储错误升格成
    /// `AppError::Storage(anyhow)`，若映射只看顶层类型，404 / 409 会静默变 500。
    #[tokio::test]
    async fn storage_semantics_survive_app_error_wrapping() {
        let cases = [
            (StorageError::NotFound("x".into()), StatusCode::NOT_FOUND),
            (
                StorageError::UniqueViolation("u".into()),
                StatusCode::CONFLICT,
            ),
            (
                StorageError::VersionConflict {
                    expected: 2,
                    actual: 1,
                },
                StatusCode::CONFLICT,
            ),
        ];
        for (error, expected) in cases {
            let app: AppError = error.into();
            let http: HttpError = app.into();
            assert_eq!(http.into_response().status(), expected);
        }
    }

    /// **另一半防退化**：领域函数返回 `anyhow::Result` 时，`?` 落进的是
    /// `AppError::Internal`（`#[error(transparent)]`），它的 `source()` 会转发给
    /// 载荷自己的 source、把载荷跳过去——从 `AppError::source()` 起走的写法在这里
    /// 会拿到一条**空链**，404 静默变 500。上一条用例走的是 `AppError::Storage`，
    /// 两个变体的 `source()` 语义不同，它盖不住这一条。
    #[tokio::test]
    async fn storage_semantics_survive_a_bare_anyhow_internal() {
        // 形状照搬真实调用栈：领域函数用 anyhow，handler 一个 `?` 收进 AppResult
        fn domain() -> anyhow::Result<()> {
            Err(StorageError::NotFound("tenant 7".into()))?;
            Ok(())
        }
        fn handler() -> Result<(), AppError> {
            domain()?;
            Ok(())
        }

        let app = handler().expect_err("领域函数必然失败");
        assert!(matches!(app, AppError::Internal(_)), "{app:?}");
        let response = HttpError::from(app).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        // 文案也要活着：退化成 500 时它会被换成 "internal error"
        assert_eq!(body_json(response).await["description"], "tenant 7");

        // 409 一侧同理，且不经 `?`（直接构造的 Internal 也在这条路上）
        let conflict = AppError::Internal(anyhow::Error::new(StorageError::VersionConflict {
            expected: 2,
            actual: 1,
        }));
        assert_eq!(
            HttpError::from(conflict).into_response().status(),
            StatusCode::CONFLICT
        );
    }

    /// 领域层加过 `anyhow::Context` 时根因在链上更深一层，仍要找得到。
    #[tokio::test]
    async fn storage_semantics_survive_added_context() {
        use anyhow::Context;

        let deep = Err::<(), _>(StorageError::NotFound("tenant 7".into()))
            .context("loading tenant")
            .context("handling request")
            .unwrap_err();
        let http: HttpError = AppError::Storage(deep).into();
        assert_eq!(http.into_response().status(), StatusCode::NOT_FOUND);
    }

    /// 非语义的存储错误（驱动 / 迁移 / 初始化）不该被误判成 4xx。
    #[tokio::test]
    async fn non_semantic_storage_errors_stay_500() {
        let app: AppError = StorageError::Init("bad dsn".into()).into();
        let http: HttpError = app.into();
        assert_eq!(
            http.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// 404 的文案取自存储层载荷，不被泛化成 "internal error"
    #[tokio::test]
    async fn wrapped_not_found_keeps_its_description() {
        let app: AppError = StorageError::NotFound("tenant 7".into()).into();
        let json = body_json(HttpError::from(app).into_response()).await;
        assert_eq!(json["code"], 404);
        assert_eq!(json["description"], "tenant 7");
    }
}
