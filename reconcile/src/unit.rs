//! 单元契约：管理器与被管理者之间的全部约定。
//!
//! 参数面收在四处：键类型、「规格是否等价」的判据、单元自身的依赖、
//! 是否设并发上限。前两处抽成关联类型与 trait 方法，后两处分别归工厂
//! 字段与管理器参数——超出这四处的差异不该存在，想扩参数面先质疑需求。
//!
//! 四处这个数字不是拍的：本框架抽自一个真实服务，那里三条运行线各手写过
//! 一份几乎逐行相同的管理器，逐行比对后真正的差异只有这四处。接入本框架的
//! 面只实现这里的契约，不再各带一套调度骨架。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio_util::sync::CancellationToken;

use {{crate_prefix_snake}}_core::util::TimestampMs;

/// 一个受管单元的**期望状态**：管理器据此决定建、停还是重建。
///
/// 实现者应当是纯数据（配置的投影），不要放连接、句柄一类的活资源——
/// 规格每轮对账都会被克隆比较。
pub trait UnitSpec: Clone + Send + Sync + 'static {
    /// 单元身份。同一管理器内唯一，且要能稳定转成字符串用于排序与日志。
    type Key: Eq + std::hash::Hash + Clone + std::fmt::Display + Send + Sync + 'static;

    fn key(&self) -> Self::Key;

    /// 影响单元**行为**的字段是否等价；返回 false 触发关旧建新。
    ///
    /// 判据要精确到「改了它就必须重连/重跑」的粒度：地址、令牌、周期属于
    /// 这一类；显示名称、备注不属于——把展示字段算进去，运维改一次名字
    /// 就会让全部单元断线重连一轮。
    fn same_as(&self, other: &Self) -> bool;
}

/// 交给单元的运行期上下文。
///
/// 只装管理器真正**拥有**的每化身状态：取消令牌与进度计数。单元的业务依赖
/// （存储、事件总线、指标、HTTP 客户端……）由工厂作为自己的字段持有、在
/// `spawn_unit` 时克隆进 future——管理器对它们一无所知，这正是形状各异的面
/// 能共用同一份骨架的原因。
pub struct UnitCtx {
    /// 管理器父令牌的子令牌：单元主循环必须把它作为 `select!` 的第一分支
    pub cancel: CancellationToken,
    progress: Arc<AtomicU64>,
}

impl UnitCtx {
    pub(crate) fn new(cancel: CancellationToken, progress: Arc<AtomicU64>) -> Self {
        Self { cancel, progress }
    }

    /// 独立上下文：给单元主体的单元测试用，不经过管理器。
    /// 令牌与进度计数都是全新的，测试可自行取消或断言进度。
    pub fn detached() -> Self {
        Self::new(CancellationToken::new(), Arc::new(AtomicU64::new(0)))
    }

    /// 声明「本化身确实干成过事」，用于清零死亡计数。
    ///
    /// # 什么时候调
    ///
    /// 由单元语义决定：轮询类单元在**完成一轮尝试**后调（成败都算——「连上了
    /// 但对端报错」也说明单元本身是活的）；长连接类单元在**进入已连接状态**后调。
    ///
    /// # 为什么不能用「spawn 成功」代替
    ///
    /// 每次都在第一轮之前 panic 的单元，若按 spawn 计进度，死亡计数永远停在
    /// 0——退避失效、放弃阈值永远够不着，它会以巡检节奏无限重建。计数器每次
    /// spawn 都换新的，所以「读数大于 0」精确等于「本化身有过进度」。
    pub fn mark_progress(&self) {
        self.progress.fetch_add(1, Ordering::Relaxed);
    }
}

/// 单元的退出方式。
#[derive(Debug)]
pub enum UnitExit {
    /// 干净收尾：收到取消信号，或活儿本身做完了。不计死亡；仍在期望集里的
    /// 干净退出单元会被无罚重建（重建频率天然被巡检节奏限住）
    Normal,
    /// 异常终止。计入死亡，触发退避与放弃判定
    Failed(anyhow::Error),
}

/// 按规格造出单元的运行体。
pub trait UnitFactory: Send + Sync + 'static {
    type Spec: UnitSpec;

    /// 返回单元的主循环。future 由管理器 `spawn`，其结束即单元死亡或收尾。
    fn spawn_unit(
        &self,
        spec: Self::Spec,
        ctx: UnitCtx,
    ) -> Pin<Box<dyn Future<Output = UnitExit> + Send>>;
}

/// 单元运行状况的只读快照（自省接口与日志用）。
#[derive(Debug, Clone)]
pub struct UnitStatus {
    pub key: String,
    pub alive: bool,
    pub deaths: u32,
    pub next_restart_at_ms: TimestampMs,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// detached 上下文开箱即用：可标进度、可取消，互不串扰
    #[tokio::test]
    async fn detached_ctx_is_self_contained() {
        let a = UnitCtx::detached();
        let b = UnitCtx::detached();
        a.mark_progress();
        assert_eq!(a.progress.load(Ordering::Relaxed), 1);
        assert_eq!(b.progress.load(Ordering::Relaxed), 0);

        a.cancel.cancel();
        assert!(a.cancel.is_cancelled());
        assert!(!b.cancel.is_cancelled());
    }
}
