//! 顶层任务面的监督者。
//!
//! 它管三件事，每件都对应一类真实踩过的坑：
//!
//! - **重名登记返回 `Result`，不是 `debug_assert!`。** 任何在 release 里会被编译掉的检查
//!   都不能充当纪律载体——那等于承认这条纪律只在开发机上成立。
//! - **收割分两段，各有各的 deadline。** `harvest` 等面自己收尾，`reap` 在 abort 之后回收。
//!   第二段必须有自己的绝对边界：一个卡在同步代码里的任务不会因为你 abort 了它就返回，
//!   无限 `join_next` 会把进程钉死在关停路径上。
//! - **收割返回结论，不返回 `()`。** "被迫强杀"和"优雅收尾"必须是调用方能区分的两件事，
//!   否则退出码只能一律是 0。

use crate::shutdown::{Stage, UnreapedTask};
use crate::task::{ExitKind, PanicSummary, PlaneError, PlaneFuture, RuntimeId, TaskExit, TaskSpec};
use std::collections::HashMap;
use std::time::Instant;
use tokio::runtime::Handle;
use tokio::task::{Id, JoinSet};

/// 注册任务时可能出现的错误。
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    /// 同一个面被登记了两次。
    ///
    /// 这在 release 下也是错误——见模块文档。
    #[error("task `{name}` is already registered on this supervisor")]
    DuplicateName {
        /// 重复的名字。
        name: crate::task::TaskName,
    },
}

/// 一次收割的结论。
///
/// 调用方据此决定退出码。**没有 `is_ok()` 这种单一布尔**：把"哪些没收回来"丢掉之后，
/// 运维手上就只剩一个"出事了"，没法定位。
#[derive(Debug, Default)]
pub struct HarvestOutcome {
    /// 这一段里收回来的全部退出记录。
    pub exits: Vec<TaskExit>,
    /// 到点仍未收回的任务。空向量意味着这一段干净收尾。
    pub unreaped: Vec<UnreapedTask>,
    /// abort 是否**真的掐到了**还在跑的任务。
    ///
    /// 注意它不是"调用过 `abort_all()`"。两者的差别直接落在退出码上：`reap` 段一律对
    /// 剩余任务调 `abort_all()`，而"剩余"里天然包含**已经自己结束、只是还没被 join 上**
    /// 的任务。按"调用过"来算的话，每一次完全干净的关停都会被记成强杀，而关停序列的六项
    /// 合取里有一条 `!forced`——一个永远为真的"被迫强杀"和没有这个结论是一回事。
    ///
    /// 判据因此取自**结果**：被掐掉的任务 join 回来是 [`ExitKind::Cancelled`]，掐了还
    /// 不回来的落进 `unreaped`。两者都没有，说明 abort 落在了一批已经结束的任务上。
    pub forced: bool,
}

impl HarvestOutcome {
    fn merge(&mut self, other: Self) {
        self.exits.extend(other.exits);
        self.unreaped.extend(other.unreaped);
        self.forced |= other.forced;
    }
}

/// 一个已登记任务的元数据。
#[derive(Clone, Copy, Debug)]
struct TaskMeta {
    spec: TaskSpec,
    runtime: RuntimeId,
}

/// 顶层任务面的监督者。
///
/// `Drop` 时会 abort 所有仍在册的任务——这是兜底，不是正常路径。正常路径是显式调用
/// [`harvest`](Self::harvest) 与 [`reap`](Self::reap)。兜底的存在理由：如果外层因为
/// panic 或早退而展开，没有它的话这些任务会脱管继续跑，而它们持有的资源（端口、连接池）
/// 也跟着一起泄漏。
#[derive(Debug)]
pub struct TaskSupervisor {
    tasks: JoinSet<Result<(), PlaneError>>,
    meta: HashMap<Id, TaskMeta>,
    registered: Vec<crate::task::TaskName>,
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSupervisor {
    /// 创建一个空的监督者。
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            meta: HashMap::new(),
            registered: Vec::new(),
        }
    }

    /// 当前仍在册（尚未被收割）的任务数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// 是否一个任务都没有。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// 在指定 runtime 上启动一个顶层任务。
    ///
    /// runtime 标识由 `handle` **派生**，调用方无法自述——传错标签导致"任务跑在 A、报告
    /// 写 B"的整类缺陷在这里不存在，因为根本没有地方能传那个标签。
    ///
    /// # Errors
    ///
    /// 同名任务重复登记时返回 [`RegisterError::DuplicateName`]。
    pub fn spawn_on(
        &mut self,
        spec: TaskSpec,
        handle: &Handle,
        future: PlaneFuture,
    ) -> Result<(), RegisterError> {
        if self.registered.contains(&spec.name) {
            return Err(RegisterError::DuplicateName { name: spec.name });
        }
        let runtime = RuntimeId::of(handle);
        let abort = self.tasks.spawn_on(future, handle);
        self.meta.insert(abort.id(), TaskMeta { spec, runtime });
        self.registered.push(spec.name);
        Ok(())
    }

    /// 等**任意一个**在册任务退出，返回它的退出记录；一个都没有了就返回 `None`。
    ///
    /// 这是给启动提交门和主编排循环用的：它们要在"等确认"或"等信号"的同时，盯住
    /// 有没有面提前死掉。没有这个入口的话，唯一能观察退出的方式是 `harvest`，而
    /// `harvest` 带 deadline、会 abort、语义是"关停"——在启动阶段调它就是把一次
    /// 正常启动伪装成一次关停。
    ///
    /// **取消安全。** 内部只 await [`JoinSet::join_next_with_id`]，它本身是取消安全的：
    /// 在 `select!` 的某一分支里被丢弃不会丢掉任何已完成任务的结果。这条性质是这个方法
    /// 能出现在 `select!` 里的全部前提，改动这一行之前先确认它还成立。
    ///
    /// 名字不叫 `next()`：`next` 会让人以为这是 [`Iterator`] 或 `Stream` 的那个 `next`，
    /// 从而以为可以 `while let Some(x) = s.next()` 地连续取到底。它不是——它每次调用
    /// 都要独占 `&mut self`，且返回的是"退出"而不是"产出"。
    pub async fn next_exit(&mut self) -> Option<TaskExit> {
        let joined = self.tasks.join_next_with_id().await?;
        Some(self.classify(joined))
    }

    /// 第一段：等需要优雅收尾的面自己结束，直到 `deadline`。
    ///
    /// 只有 [`ShutdownClass::Graceful`] 的面值得等。一旦所有 `Graceful` 面都已退出，
    /// 本段**立即返回**，把剩下的预算留给后面几段——预算是绝对时间，省下来的是真的。
    ///
    /// 返回前会把**已经自己结束**的其余任务一并收掉，理由见函数体里那段注释：不收的后果
    /// 不是少几条记录，是结论错。
    ///
    /// [`ShutdownClass::Graceful`]: crate::task::ShutdownClass::Graceful
    pub async fn harvest(&mut self, deadline: Instant) -> HarvestOutcome {
        let mut outcome = HarvestOutcome::default();

        while self.outstanding_graceful() > 0 {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match tokio::time::timeout(remaining, self.tasks.join_next_with_id()).await {
                // 超时：本段到点，剩下的交给 reap。
                Err(_elapsed) => break,
                // JoinSet 空了。
                Ok(None) => break,
                Ok(Some(joined)) => outcome.exits.push(self.classify(joined)),
            }
        }

        // Graceful 面都回来了，但别的面**多半也已经自己停下来了**——它们看的是同一个取消
        // 令牌，而这一段窗口里进程一直在跑。这里把它们收掉。
        //
        // 这一步不花预算：`try_join_next_with_id` 不是 `async`，「没有已完成的任务、或者
        // 集合为空，就返回 `None`」（tokio 1.53.1 该方法文档原话）。它只是把"已经躺在那儿
        // 的结果"取走，不会为任何一个还在跑的任务多等一个 poll。
        //
        // 不收的后果不是少几条记录，是**结论错**：留在册上的任务会让下一段看起来"还有东西
        // 要强杀"，于是每一次干净关停都发一次 `Phase::Forcing`，并把 `forced` 记成真。
        while let Some(joined) = self.tasks.try_join_next_with_id() {
            outcome.exits.push(self.classify(joined));
        }

        // 到点仍未收尾的 Graceful 面要记下来——它们下一段会被 abort，
        // 但"它是在 harvest 段卡住的"这个信息只有现在才知道。
        for meta in self.outstanding_meta() {
            if meta.spec.class.gets_harvest_budget() && Instant::now() >= deadline {
                outcome.unreaped.push(UnreapedTask {
                    name: Some(meta.spec.name),
                    runtime: meta.runtime,
                    class: meta.spec.class,
                    stalled_at: Stage::Harvest,
                });
            }
        }

        outcome
    }

    /// 第二段：abort 全部剩余任务，然后在**自己的** `deadline` 内回收。
    ///
    /// 到点即放弃。`abort()` 只能取消停在 `.await` 点上的任务；一个跑在
    /// `spawn_blocking` 里或陷在同步循环里的任务不会因为被 abort 而返回，
    /// 无界地等它就是把进程永久钉在关停路径上。
    pub async fn reap(&mut self, deadline: Instant) -> HarvestOutcome {
        let mut outcome = HarvestOutcome::default();
        if self.tasks.is_empty() {
            return outcome;
        }

        self.tasks.abort_all();

        // `checked_duration_since` 返回 `None` 就是"deadline 已经过了"：循环条件本身
        // 就是那条绝对边界，不需要在循环体里再判一次。
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match tokio::time::timeout(remaining, self.tasks.join_next_with_id()).await {
                Err(_elapsed) => break,
                Ok(None) => break,
                Ok(Some(joined)) => outcome.exits.push(self.classify(joined)),
            }
        }

        // 真正没回来的：abort 了、等到点了、还是没 join 上。
        for meta in self.outstanding_meta() {
            outcome.unreaped.push(UnreapedTask {
                name: Some(meta.spec.name),
                runtime: meta.runtime,
                class: meta.spec.class,
                stalled_at: Stage::Reap,
            });
        }

        // 结论取自结果，不取自"我调用了什么"——理由见 [`HarvestOutcome::forced`]。
        // `abort_all()` 落在一批已经结束的任务上不算强杀；真被掐掉的回来是 `Cancelled`，
        // 掐了还不回来的在 `unreaped` 里。
        outcome.forced = outcome
            .exits
            .iter()
            .any(|exit| matches!(exit.kind, ExitKind::Cancelled))
            || !outcome.unreaped.is_empty();

        outcome
    }

    /// 走完两段收割，返回合并后的结论。
    pub async fn shutdown(
        &mut self,
        harvest_deadline: Instant,
        reap_deadline: Instant,
    ) -> HarvestOutcome {
        let mut outcome = self.harvest(harvest_deadline).await;
        // harvest 段登记的 stalled 记录会被 reap 段的结论取代：一个面要么最终回来了
        // （于是它根本不该出现在 unreaped 里），要么卡在 reap 段。只留最终结论。
        outcome.unreaped.clear();
        outcome.merge(self.reap(reap_deadline).await);
        outcome
    }

    /// 把 `JoinSet` 的产出翻译成 [`TaskExit`]。
    ///
    /// `JoinError` 的 `is_panic()` / `is_cancelled()` 区分必须保留到这一步。把它
    /// `format!` 成字符串会让 panic 与外部取消变得无法分辨，而这两者对退出码的含义完全不同。
    fn classify(
        &mut self,
        joined: Result<(Id, Result<(), PlaneError>), tokio::task::JoinError>,
    ) -> TaskExit {
        match joined {
            Ok((id, result)) => {
                let meta = self.meta.remove(&id);
                TaskExit {
                    name: meta.map(|m| m.spec.name),
                    runtime: meta.map(|m| m.runtime),
                    kind: match result {
                        Ok(()) => ExitKind::Returned,
                        Err(err) => ExitKind::Failed(err),
                    },
                }
            }
            Err(join_err) => {
                let id = join_err.id();
                let meta = self.meta.remove(&id);
                // 先判 panic 再判 cancel：两者互斥，但顺序写死能让阅读者不必猜。
                let kind = if join_err.is_panic() {
                    ExitKind::Panicked(PanicSummary::from_payload(join_err.into_panic().as_ref()))
                } else {
                    ExitKind::Cancelled
                };
                TaskExit {
                    name: meta.map(|m| m.spec.name),
                    runtime: meta.map(|m| m.runtime),
                    kind,
                }
            }
        }
    }

    fn outstanding_graceful(&self) -> usize {
        self.outstanding_meta()
            .filter(|m| m.spec.class.gets_harvest_budget())
            .count()
    }

    fn outstanding_meta(&self) -> impl Iterator<Item = TaskMeta> + '_ {
        self.meta.values().copied()
    }
}

impl Drop for TaskSupervisor {
    fn drop(&mut self) {
        // 兜底：外层展开时不留脱管任务。
        // 这里不能 await，所以只 abort 不回收——回收是正常路径的事。
        if !self.tasks.is_empty() {
            self.tasks.abort_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{ShutdownClass, TaskName};
    use std::sync::Arc;
    use std::time::Duration;

    fn spec(name: TaskName, class: ShutdownClass) -> TaskSpec {
        TaskSpec::new(name, class)
    }

    fn ready_plane() -> PlaneFuture {
        Box::pin(async { Ok(()) })
    }

    #[tokio::test]
    async fn duplicate_task_name_is_a_runtime_error() {
        // 这条在 release 下也必须成立——所以是 Result，不是 debug_assert!。
        let mut sup = TaskSupervisor::new();
        let handle = Handle::current();
        sup.spawn_on(
            spec(TaskName::Http, ShutdownClass::Graceful),
            &handle,
            ready_plane(),
        )
        .expect("首次登记应当成功");
        let err = sup
            .spawn_on(
                spec(TaskName::Http, ShutdownClass::Graceful),
                &handle,
                ready_plane(),
            )
            .expect_err("重名登记必须失败");
        assert!(matches!(err, RegisterError::DuplicateName { name } if name == TaskName::Http));
    }

    #[tokio::test]
    async fn empty_supervisor_exits_normally() {
        let mut sup = TaskSupervisor::new();
        let now = Instant::now();
        let outcome = sup
            .shutdown(
                now + Duration::from_millis(50),
                now + Duration::from_millis(100),
            )
            .await;
        assert!(outcome.exits.is_empty());
        assert!(outcome.unreaped.is_empty());
        assert!(!outcome.forced, "空 supervisor 不该被记为强杀");
    }

    #[tokio::test]
    async fn returned_and_failed_are_distinguishable() {
        let mut sup = TaskSupervisor::new();
        let handle = Handle::current();
        sup.spawn_on(
            spec(TaskName::Http, ShutdownClass::Graceful),
            &handle,
            ready_plane(),
        )
        .unwrap();
        sup.spawn_on(
            spec(TaskName::Ticker, ShutdownClass::Graceful),
            &handle,
            Box::pin(async { Err(PlaneError::failed(TaskName::Ticker, "boom")) }),
        )
        .unwrap();

        let now = Instant::now();
        let outcome = sup.harvest(now + Duration::from_secs(5)).await;
        assert_eq!(outcome.exits.len(), 2);

        let http = outcome
            .exits
            .iter()
            .find(|e| e.name == Some(TaskName::Http))
            .unwrap();
        let ticker = outcome
            .exits
            .iter()
            .find(|e| e.name == Some(TaskName::Ticker))
            .unwrap();
        assert!(
            matches!(http.kind, ExitKind::Returned),
            "正常返回不该被记成失败"
        );
        assert!(
            matches!(ticker.kind, ExitKind::Failed(_)),
            "返回 Err 必须落到 Failed"
        );
    }

    #[tokio::test]
    async fn panic_is_classified_as_panicked_not_cancelled() {
        let mut sup = TaskSupervisor::new();
        let handle = Handle::current();
        sup.spawn_on(
            spec(TaskName::Http, ShutdownClass::Graceful),
            &handle,
            Box::pin(async { panic!("plane exploded") }),
        )
        .unwrap();

        let outcome = sup.harvest(Instant::now() + Duration::from_secs(5)).await;
        assert_eq!(outcome.exits.len(), 1);
        match &outcome.exits[0].kind {
            ExitKind::Panicked(summary) => {
                assert_eq!(summary.message(), Some("plane exploded"));
            }
            other => panic!("panic 必须分类为 Panicked，实际是 {}", other.as_str()),
        }
    }

    #[tokio::test]
    async fn abortable_task_is_not_waited_for_in_harvest() {
        // Abortable 面永不自行结束；harvest 段不该为它消耗任何预算。
        let mut sup = TaskSupervisor::new();
        let handle = Handle::current();
        sup.spawn_on(
            spec(TaskName::Ticker, ShutdownClass::Abortable),
            &handle,
            Box::pin(async {
                std::future::pending::<()>().await;
                Ok(())
            }),
        )
        .unwrap();

        let started = Instant::now();
        let outcome = sup.harvest(started + Duration::from_secs(30)).await;
        let spent = started.elapsed();

        assert!(outcome.exits.is_empty());
        assert!(
            spent < Duration::from_secs(1),
            "harvest 为 Abortable 面等了 {spent:?}——预算被浪费了"
        );
    }

    /// 一个看着"取消令牌"自己停下来的面。
    ///
    /// 用 `watch` 而不是 `CancellationToken`：`core` 没有 `tokio-util`（理由写在
    /// `core/Cargo.toml` 里），而这条用例要证的事情与令牌的具体类型无关。
    fn cooperative(mut stop: tokio::sync::watch::Receiver<bool>) -> PlaneFuture {
        Box::pin(async move {
            // 先看当前值再等变化：取消可能发生在这个面第一次被 poll **之前**，
            // 只写 `changed().await` 的话它会永远等下去。
            while !*stop.borrow_and_update() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
            Ok(())
        })
    }

    #[tokio::test]
    async fn a_cooperative_shutdown_is_not_reported_as_forced() {
        // 回归用例：`reap` 段一律对剩余任务调 `abort_all()`，而 Abortable 面按约定
        // 拿不到 harvest 预算——它在 harvest 返回时**必然**还在册上。如果 `forced` 按
        // "调用过 abort" 算，那么每一次干净关停都会被记成强杀，`RunReport::succeeded()`
        // 的六项合取永远为假，退出码永远非零。一条永远为真的"被迫强杀"等于没有这个结论。
        let (stop, rx) = tokio::sync::watch::channel(false);
        let mut sup = TaskSupervisor::new();
        let handle = Handle::current();

        sup.spawn_on(
            spec(TaskName::Http, ShutdownClass::Graceful),
            &handle,
            cooperative(rx.clone()),
        )
        .unwrap();
        sup.spawn_on(
            spec(TaskName::Ticker, ShutdownClass::Abortable),
            &handle,
            cooperative(rx),
        )
        .unwrap();

        stop.send(true).unwrap();

        let now = Instant::now();
        let outcome = sup
            .shutdown(now + Duration::from_secs(5), now + Duration::from_secs(5))
            .await;

        assert_eq!(outcome.exits.len(), 2, "两个面都必须留下退出记录");
        assert!(
            outcome
                .exits
                .iter()
                .all(|exit| matches!(exit.kind, ExitKind::Returned)),
            "两个面都是自己停下来的，不该出现 Cancelled"
        );
        assert!(outcome.unreaped.is_empty());
        assert!(
            !outcome.forced,
            "没有任何一个面是被掐掉的——这一次关停不是强杀"
        );
    }

    // 这条用例**必须**跑在多线程调度器上，而其余用例默认的 current-thread 就够。
    // 理由是用例本身的机制：它用一个同步阻塞的任务来制造"abort 不生效"的局面，而在
    // current-thread 上那次阻塞会占住唯一的工作线程——连 `reap` 自己都排不上队。等它
    // 睡完，任务已经正常退出，`unreaped` 自然是空的，用例于是测了个反面。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reap_gives_up_at_its_own_deadline() {
        // 回归用例：abort 不能取消同步阻塞的任务，所以 reap 必须有自己的绝对边界。
        let mut sup = TaskSupervisor::new();
        let handle = Handle::current();
        sup.spawn_on(
            spec(TaskName::Ticker, ShutdownClass::Abortable),
            &handle,
            Box::pin(async {
                // 同步阻塞：abort() 对它无效。
                // 只睡 1s 而不是 10s：它要活过 200ms 的 reap deadline（5 倍余量足够），
                // 而 runtime 在 drop 时会等这个工作线程回来——多出来的那 9s 全是门禁的等待。
                std::thread::sleep(Duration::from_secs(1));
                Ok(())
            }),
        )
        .unwrap();

        // 让任务真正开始跑，否则 abort 会在它启动前生效。
        tokio::time::sleep(Duration::from_millis(50)).await;

        let started = Instant::now();
        let outcome = sup.reap(started + Duration::from_millis(200)).await;
        let spent = started.elapsed();

        assert!(
            spent < Duration::from_secs(5),
            "reap 等了 {spent:?}——它没有遵守自己的 deadline"
        );
        assert!(outcome.forced, "abort 掐到了没回来的任务，必须记为 forced");
        assert_eq!(
            outcome.unreaped.len(),
            1,
            "没收回来的任务必须留下结构化记录"
        );
        assert_eq!(outcome.unreaped[0].stalled_at, Stage::Reap);
        assert_eq!(outcome.unreaped[0].name, Some(TaskName::Ticker));
    }

    #[tokio::test]
    async fn drop_aborts_outstanding_tasks() {
        // 用存活计数反证：guard 被 drop 说明任务确实没继续跑。
        let alive = Arc::new(());
        let weak = Arc::downgrade(&alive);

        {
            let mut sup = TaskSupervisor::new();
            let handle = Handle::current();
            sup.spawn_on(
                spec(TaskName::Ticker, ShutdownClass::Abortable),
                &handle,
                Box::pin(async move {
                    let _held = alive;
                    std::future::pending::<()>().await;
                    Ok(())
                }),
            )
            .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            // sup 在这里离开作用域。
        }

        // 给 abort 一点时间生效。
        for _ in 0..50 {
            if weak.upgrade().is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("supervisor drop 之后任务仍然持有资源——存在脱管任务");
    }

    #[tokio::test]
    async fn next_exit_on_empty_supervisor_is_none() {
        // 提交门靠这个 `None` 区分"还没人退出"和"已经没人可等了"。
        // 如果它改成永久 pending，一个空的任务面会把启动卡死在门上。
        let mut sup = TaskSupervisor::new();
        assert!(sup.next_exit().await.is_none());
    }

    #[tokio::test]
    async fn next_exit_reports_a_failing_plane_before_any_deadline() {
        let mut sup = TaskSupervisor::new();
        sup.spawn_on(
            spec(TaskName::Ticker, ShutdownClass::Abortable),
            &Handle::current(),
            Box::pin(async { Err(PlaneError::failed(TaskName::Ticker, "boom")) }),
        )
        .unwrap();

        let exit = sup
            .next_exit()
            .await
            .expect("面已经失败，必须能取到退出记录");
        assert_eq!(exit.name, Some(TaskName::Ticker));
        assert!(matches!(exit.kind, ExitKind::Failed(_)));
        // 取走之后 supervisor 就空了：退出记录只被观察一次。
        assert!(sup.is_empty());
    }

    #[tokio::test]
    async fn next_exit_survives_being_dropped_inside_select() {
        // 这条测的是取消安全：`next_exit` 在 select! 里输给了另一条分支、被丢弃，
        // 已完成任务的结果**不能**跟着丢。提交门每收一次确认就会重进一次 select!，
        // 只要这条不成立，一个在门上退出的面就会被永久漏掉。
        let mut sup = TaskSupervisor::new();
        sup.spawn_on(
            spec(TaskName::Http, ShutdownClass::Graceful),
            &Handle::current(),
            ready_plane(),
        )
        .unwrap();

        // 让任务先真正跑完，再进 select!——否则这一轮谁赢是调度器说了算，
        // 断言就变成了偶然成立。
        tokio::time::sleep(Duration::from_millis(50)).await;

        let loser = tokio::select! {
            biased;
            () = std::future::ready(()) => true,
            _ = sup.next_exit() => false,
        };
        assert!(loser, "biased 下第一条分支必须先被选中");

        let exit = sup.next_exit().await.expect("上一轮被丢弃的结果不该丢失");
        assert_eq!(exit.name, Some(TaskName::Http));
        assert!(matches!(exit.kind, ExitKind::Returned));
    }
}
