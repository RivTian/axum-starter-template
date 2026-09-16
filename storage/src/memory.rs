//! 进程内的门面实现。`#[cfg(feature = "test-utils")]`，默认不编译。
//!
//! # 它为什么存在
//!
//! 门面纪律的前三条约束都是**否定式**的证据——"泄漏了会红"。这一条是唯一的**肯定式**证据：
//! 门面确实支撑得起两个形态完全不同的后端。一个只有单实现的 trait，无论纪律写得多严，
//! 都无法排除"这个 trait 其实是照着 SQLite 的形状长出来的"。
//!
//! 第二个收益同样实在：`StorageError::Unavailable` 与 `Internal` 在 SQLite 实现里只能靠
//! 制造真故障来触发（关池、改校验和），而上层要验证降级行为时需要的是**随时可切换**的故障。
//!
//! # 它刻意不做的事
//!
//! - **不存任何数据。** 门面上只有 `health()`，存数据没有消费者。等第一个业务方法进门面时
//!   再说——那时它需要什么形状，现在猜不出来。
//! - **不引入任何依赖。** 只用 `std`。一个默认不编译的模块把新依赖拖进 `Cargo.toml`，
//!   会让默认 feature 下的编译闭包断言变得难解释。

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

use service_core::storage::{Storage, StorageError, StorageFuture};

/// 后端名。与 SQLite 的那个并列，不共用常量：它们是两个后端，名字碰巧都要有而已。
const BACKEND: &str = "memory";

const HEALTHY: u8 = 0;
const FAULT_UNAVAILABLE: u8 = 1;
const FAULT_INTERNAL: u8 = 2;

/// 一个进程内的 [`Storage`]，可以随时注入故障。
///
/// 故障状态用 `AtomicU8` 而不是 `Mutex`：`health()` 里要读它，而 `health()` 返回的是一个
/// `Future`——拿着 `MutexGuard` 跨 `.await` 会被 `clippy::await_holding_lock`（本工作区设为
/// `deny`）判红。原子量没有这个问题，也没有中毒锁的问题。
///
/// 用法见本模块的 `faults_can_be_injected_and_taken_back`。这里**不写 doctest**：
/// doctest 里的 `use` 必须写 crate 的真名，而本模板的 crate 名由模板变量展开而来，
/// 每个使用者展开出的名字都不同——写死任何一个都会在别人展开之后立刻失效。
/// 这条对整个模板成立：全模板 doctest 计数为 0，示例代码一律标 `text` 围栏
/// （标 `ignore` 仍然会被计成一条 doctest，理由写在 `testkit` 的 `log` 模块）。
pub struct InMemoryStorage {
    fault: AtomicU8,
}

impl InMemoryStorage {
    /// 一个健康的实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            fault: AtomicU8::new(HEALTHY),
        }
    }

    /// 之后的 `health()` 一律报 [`StorageError::Unavailable`]（上层应当降级成 503）。
    pub fn fail_unavailable(&self) {
        self.fault.store(FAULT_UNAVAILABLE, Ordering::SeqCst);
    }

    /// 之后的 `health()` 一律报 [`StorageError::Internal`]（上层应当报 500）。
    pub fn fail_internal(&self) {
        self.fault.store(FAULT_INTERNAL, Ordering::SeqCst);
    }

    /// 恢复健康。
    ///
    /// 有这个方法，是因为"探活恢复之后服务要重新变 ready"本身就是一条要验的行为——
    /// 只能注入、不能恢复的假实现验不了它。
    pub fn recover(&self) {
        self.fault.store(HEALTHY, Ordering::SeqCst);
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for InMemoryStorage {
    /// 与 SQLite 实现同一条纪律：只输出后端名。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(BACKEND)
    }
}

impl Storage for InMemoryStorage {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>> {
        // 注意这里**没有** `.await`：`Box::pin` 的是一个立刻就绪的 `async` 块。假实现要能
        // 在 `tokio::time::pause()` 之下用，就不能自己引入等待。
        Box::pin(async move {
            match self.fault.load(Ordering::SeqCst) {
                HEALTHY => Ok(()),
                FAULT_UNAVAILABLE => Err(StorageError::Unavailable { backend: BACKEND }),
                _ => Err(StorageError::Internal {
                    context: Box::from("injected fault from the in-memory storage"),
                }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// 一次注入动作。抽成别名是 `clippy::type_complexity` 逼出来的，但结果更好读：
    /// 表驱动用例的第二列就是"对夹具做什么"。
    type Inject = fn(&InMemoryStorage);

    /// 表驱动的一行：名字、做什么、期望的错误种类（`None` = 应当健康）。
    type Case = (&'static str, Inject, Option<&'static str>);

    #[tokio::test]
    async fn faults_can_be_injected_and_taken_back() {
        let fake = Arc::new(InMemoryStorage::new());
        let storage: Arc<dyn Storage> = Arc::clone(&fake) as Arc<dyn Storage>;

        let cases: &[Case] = &[
            ("initially healthy", |_| {}, None),
            (
                "unavailable",
                InMemoryStorage::fail_unavailable,
                Some("unavailable"),
            ),
            ("internal", InMemoryStorage::fail_internal, Some("internal")),
            ("recovered", InMemoryStorage::recover, None),
        ];

        for (i, (name, apply, want)) in cases.iter().enumerate() {
            apply(&fake);
            let got = storage.health().await;
            match (want, got) {
                (None, Ok(())) => {}
                (Some(kind), Err(err)) => {
                    assert_eq!(err.kind_str(), *kind, "TC{i} ({name}) 错误种类不对");
                }
                (None, Err(err)) => panic!("TC{i} ({name}) 应当健康，实际 {err}"),
                (Some(kind), Ok(())) => panic!("TC{i} ({name}) 应当报 {kind}，实际健康"),
            }
        }
    }

    #[test]
    fn debug_shows_the_backend_name_only() {
        assert_eq!(format!("{:?}", InMemoryStorage::new()), BACKEND);
    }
}
