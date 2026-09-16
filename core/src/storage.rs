//! 存储门面。
//!
//! **trait 和错误类型定义在 `core`，实现放在 `storage` crate。**
//!
//! 这一条不是洁癖。它让 `api` 能持有 `Arc<dyn Storage>` 而**不依赖 `storage`**——
//! 于是"单向分层"和"门面不泄漏后端细节"变成同一件事的两面，由依赖图强制，
//! 而不是靠约定。trait 和实现一旦放在同一个 crate，展示层就会直接依赖存储实现，
//! 之后任何一次"顺手 `use` 一下具体类型"都没有人能发现。

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// 门面方法的返回类型。
///
/// 用装箱 future 而不是 `async fn in trait`：后者目前还不能让 trait 成为对象安全的，
/// 而 `Arc<dyn Storage>` 是这套设计的核心——没有它，`api` 就必须泛型化到后端类型上，
/// 分层立刻塌掉。装箱的代价是每次调用一次分配，对存储操作可以忽略。
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 存储门面。
///
/// 模板里只有 `health` 一个方法——因为模板里没有业务。加业务方法时保持同样的形状：
/// 参数和返回值都是 `core` 里的类型，**不出现任何后端类型**。一旦某个方法的签名里出现了
/// `sqlx::` 开头的东西，这个 trait 就不再是门面了。
pub trait Storage: Send + Sync + fmt::Debug {
    /// 存储是否可用。
    ///
    /// `/readyz` 用它判断就绪。实现必须是**有界**的——调用方会给它套超时，
    /// 但一个永远不返回的实现会把超时变成唯一的退出途径，那就等于没有健康检查。
    ///
    /// # Errors
    ///
    /// 后端不可用、连接耗尽或探测本身失败时返回 [`StorageError`]。
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>>;
}

/// 迁移所处的阶段。用于把迁移失败定位到具体环节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationStage {
    /// 读取内嵌的迁移集合。
    Load,
    /// 校验已应用迁移与内嵌集合是否一致（校验和／顺序）。
    Validate,
    /// 实际执行。
    Apply,
}

impl MigrationStage {
    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::Validate => "validate",
            Self::Apply => "apply",
        }
    }
}

/// 存储层的归一化错误。
///
/// **不含 SQL 语句，不含连接串，不含凭据。** 这些东西一旦进了错误类型，就迟早会进日志，
/// 然后进日志聚合系统。`Internal` 的 `context` 是 `Box<str>` 且由实现方**手工构造**，
/// 不是把上游错误 `to_string()` 一把梭。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// 后端连不上或已不可用。
    #[error("storage backend `{backend}` is unavailable")]
    Unavailable {
        /// 后端名（`&'static str`：用户输入进不来）。
        backend: &'static str,
    },

    /// 迁移失败。
    #[error("migration failed at stage `{}`: {hint}", stage.as_str())]
    Migration {
        /// 失败发生在哪一步。
        stage: MigrationStage,
        /// 可以安全记录的提示（`&'static str`，不含用户数据）。
        hint: &'static str,
    },

    /// 目标不存在。
    #[error("not found")]
    NotFound,

    /// 与当前状态冲突。
    #[error("conflict")]
    Conflict,

    /// 违反唯一约束。
    #[error("unique constraint `{constraint}` violated")]
    UniqueViolation {
        /// 约束名。来自数据库元数据，不是用户输入的原文。
        constraint: Box<str>,
    },

    /// 乐观锁版本冲突。
    ///
    /// 模板里**没有构造点**——因为模板里没有带版本号的业务实体。保留它是为了让
    /// `api` 的状态码映射从第一天起就覆盖这一类；本模板对它显式标注
    /// 「无自动化证据」，而不是写一个假装验证过它的测试。
    #[error("version conflict")]
    VersionConflict,

    /// 其余内部错误。
    #[error("internal storage error: {context}")]
    Internal {
        /// 可以安全记录的说明。
        context: Box<str>,
    },
}

impl StorageError {
    /// 报告与日志里使用的稳定短名（不含任何负载）。
    #[must_use]
    pub const fn kind_str(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::Migration { .. } => "migration",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::UniqueViolation { .. } => "unique_violation",
            Self::VersionConflict => "version_conflict",
            Self::Internal { .. } => "internal",
        }
    }
}

/// 关闭存储的结果。**三态 + 原因。**
///
/// **刻意没有 `NotCreated`。** 有这么一个变体的话，"池根本没建起来"和"池建起来了但
/// 没敢关"共用一条正常路径，退出码分不出这两件事。这里靠类型消掉它：`close` 是
/// `StorageOwner` 的方法，而 `StorageOwner` 只由成功的 `open` 产出——没建成池就没有
/// owner，也就没有地方能调用 `close`。一个构造不出来的变体不该存在。
///
/// 关闭超时归入 [`Self::Failed`]（见 [`CloseOutcome::timed_out`]）：从调用方的角度，
/// "到点了还没关上"和"关的时候报错了"是同一个结论——**没有证据说明池已经关上了**。
#[derive(Debug)]
pub enum CloseOutcome {
    /// 池已正常关闭。
    Closed,
    /// **举不出"没有面还在用它"的证据**，因此没有关。
    ///
    /// 这不是失败，也不是成功：它是一个"需要人来看"的结论。把它并进 `Closed` 会让报告
    /// 说谎，并进 `Failed` 会让运维去查一个并不存在的错误。
    SkippedUnproven {
        /// 为什么举不出证据。`&'static str`：原因是代码里枚举出来的几种，不是运行期拼的。
        reason: &'static str,
    },
    /// 关闭过程本身失败（含到点未完成）。
    Failed(StorageError),
}

impl CloseOutcome {
    /// 到点仍未关闭。
    ///
    /// 措辞只有这一处，报告里出现的字样因此是稳定的。
    #[must_use]
    pub fn timed_out() -> Self {
        Self::Failed(StorageError::Internal {
            context: Box::from("close did not complete before its deadline"),
        })
    }

    /// 是否算作"干净地关掉了"。
    ///
    /// 无 `_` 臂：新增结局时必须显式回答它算不算成功。`RunReport::succeeded()` 的六项
    /// 合取里有一项就是它。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        match self {
            Self::Closed => true,
            Self::SkippedUnproven { .. } | Self::Failed(_) => false,
        }
    }

    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::SkippedUnproven { .. } => "skipped_unproven",
            Self::Failed(_) => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_carry_no_payload_beyond_safe_fields() {
        // 这条用例守的是"错误消息不会把 SQL / 凭据带进日志"。
        // 它能守住的部分：类型上就没有能放下 SQL 的字段。
        let cases: Vec<(StorageError, &str)> = vec![
            (
                StorageError::Unavailable { backend: "sqlite" },
                "unavailable",
            ),
            (
                StorageError::Migration {
                    stage: MigrationStage::Apply,
                    hint: "checksum mismatch",
                },
                "migration",
            ),
            (StorageError::NotFound, "not_found"),
            (StorageError::Conflict, "conflict"),
            (
                StorageError::UniqueViolation {
                    constraint: Box::from("idx_users_email"),
                },
                "unique_violation",
            ),
            (StorageError::VersionConflict, "version_conflict"),
            (
                StorageError::Internal {
                    context: Box::from("pool closed"),
                },
                "internal",
            ),
        ];
        for (i, (err, expected)) in cases.iter().enumerate() {
            assert_eq!(err.kind_str(), *expected, "TC{i} ({expected}) 短名不符");
            assert!(
                !err.to_string().is_empty(),
                "TC{i} ({expected}) 的 Display 不该为空"
            );
        }
    }

    #[test]
    fn only_a_confirmed_close_counts_as_clean() {
        assert!(CloseOutcome::Closed.is_clean());
        assert!(
            !CloseOutcome::SkippedUnproven {
                reason: "http plane never reported an exit"
            }
            .is_clean(),
            "举不出证据就没关的情况不能算 clean——它需要人来看"
        );
        assert!(
            !CloseOutcome::timed_out().is_clean(),
            "到点未关上没有任何证据说明池已经关上了"
        );
    }

    #[test]
    fn timed_out_is_a_failure_with_a_stable_wording() {
        match CloseOutcome::timed_out() {
            CloseOutcome::Failed(StorageError::Internal { context }) => {
                assert!(context.contains("deadline"), "超时措辞必须提到 deadline");
            }
            other => panic!("超时必须归入 Failed，实际是 {}", other.as_str()),
        }
    }
}
