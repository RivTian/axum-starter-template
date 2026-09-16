//! 运行报告与退出码。
//!
//! # 一个进程只有一个地方决定退出码
//!
//! `run` 返回 [`RunReport`]，`main` 把它交给 [`exit_code`]。中间没有任何一处
//! `std::process::exit`。这条规则的收益不是洁癖：`exit` 会绕过 `Drop`，而关停序列的最后
//! 一段（关 runtime）恰好发生在 `block_on` 返回**之后**——在那之前 `exit` 掉，等于把
//! 前面三段预算全做成了白工，而日志上完全看不出来。
//!
//! # 为什么 `succeeded()` 是六项合取而不是一个布尔字段
//!
//! 「干净地停下来了」是个复合结论，它的每一项都对应一类真实的、能单独发生的故障：
//!
//! | 项 | 不成立时说明 |
//! | --- | --- |
//! | ① 触发源是信号 | 进程不是被要求停的，是自己死的（面退出 / 面全没了 / 信号流断了） |
//! | ② 没有动用过强制手段 | 有任务不肯在 harvest 段自己返回，是被 `abort()` 掉的 |
//! | ③ 预算计划有效 | 配置里的时长溢出了，关停是在一份 fail-closed 的计划上跑的 |
//! | ④ 没有收不回来的任务 | 有任务到点仍未回收——它可能还占着端口或连接 |
//! | ⑤ 存储干净关闭 | 池没关上，或者举不出"没人还在用它"的证据 |
//! | ⑥ 所有任务都是 `Returned` | 有面 panic、返回了错误，或是被 abort 取消的 |
//!
//! 把它们压成一个 `bool` 字段并在某处 `report.ok = false`，等于把"是哪一项不成立"的
//! 信息在写入的那一刻就丢了。这里反过来：六项各自是可读的数据，结论由它们**算**出来。
//! 这也正是「如实上报，不假报 graceful」的做法——假报几乎总是因为结论被提前固化。

use std::process::ExitCode;

use service_core::shutdown::UnreapedTask;
use service_core::storage::CloseOutcome;
use service_core::task::{ExitKind, TaskExit};

use crate::lifecycle::stop::StopCause;

/// 一次 `run` 的完整结论。
#[derive(Debug)]
pub struct RunReport {
    outcome: Outcome,
}

/// 这次运行到底属于哪一类。
///
/// 分支而不是给 [`Stopped`] 加一个 `startup_error: Option<_>` 字段：一次没启动起来的
/// 运行**没有**退出记录、没有触发源、没有关池结果，让它和正常关停共用一个结构，就得给
/// 那些字段编造空值，而空值和"真的一个任务都没有"长得一模一样。
#[derive(Debug)]
enum Outcome {
    /// 压根没跑起来。
    StartupFailed(StartupFailure),
    /// 跑起来了，然后停了。
    Stopped(Stopped),
    /// 没打算跑：`--help` / `--version` 打印完就走。
    ///
    /// 第三支是被 `--help` 逼出来的，不是预铺的形状。它本可以塞进 `Stopped`——
    /// `cause: None`、三个空 `Vec`、一个假的 `CloseOutcome::Closed`——代价是
    /// [`Stopped::succeeded`] 的第①项（"触发源是信号"）会判它失败，于是
    /// `prog --help` 的退出码变成 1。要救回来就得在那六项合取里加一个"除非其实没跑"的
    /// 例外，而那正是本文件开头说的"把结论提前固化"。多一支反而让六项合取保持干净：
    /// 它只回答「跑过的那些运行停得干不干净」。
    Printed {
        /// 打印的是哪一个。只进日志与用例断言，不进退出码。
        action: &'static str,
    },
}

/// 启动失败。
#[derive(Debug)]
pub struct StartupFailure {
    kind: &'static str,
    message: String,
}

impl StartupFailure {
    /// 失败的大类。取值来自各错误类型自己的 `kind_str()`，是代码里枚举出来的常量。
    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// 渲染好的失败原因（含 `source` 链）。
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// 一次真正跑起来又停下来的运行。
#[derive(Debug)]
struct Stopped {
    cause: Option<StopCause>,
    exits: Vec<TaskExit>,
    unreaped: Vec<UnreapedTask>,
    forced: bool,
    plan_invalid: bool,
    storage_close: CloseOutcome,
}

impl Stopped {
    /// 六项合取。**顺序与模块文档那张表逐项对应**，改动时两边一起改。
    fn succeeded(&self) -> bool {
        // ① 触发源是信号。
        self.cause.as_ref().is_some_and(StopCause::is_expected)
            // ② abort 没有掐到任何还在跑的面。**不是**"没调用过 `abort_all()`"——`reap`
            //    段一律要调它，而那一刻 Abortable 面还在册上，那样写等于恒假。
            //    这一条在结论上被 ⑥ 与 ④ 蕴含（被掐掉的回来是 `Cancelled`，掐了还不回来
            //    的进 `unreaped`）。留着它不是为了多一道判断，是因为它是 supervisor 自己
            //    给出的结论，也是 `shutdown_forced` 那条日志的取值来源——退出码与日志由
            //    同一个布尔驱动，就不会出现"日志说强杀了、退出码说一切正常"。
            && !self.forced
            // ③ 预算计划本身有效（cleanup_failed 的定义就是 `Plan { invalid: true }`）。
            && !self.plan_invalid
            // ④ 没有到点未回收的任务。
            && self.unreaped.is_empty()
            // ⑤ 存储干净关闭。
            && self.storage_close.is_clean()
            // ⑥ 所有任务都是正常返回。`Cancelled` 不算：它意味着这个面是被掐掉的，
            //    哪怕整个过程在预算之内，它也没有机会跑完自己的收尾代码。
            && self
                .exits
                .iter()
                .all(|exit| matches!(exit.kind, ExitKind::Returned))
    }
}

impl RunReport {
    /// 启动阶段就失败了。
    ///
    /// `kind` 用各错误类型自己的稳定短名，`error` 会连着 `source` 链一起渲染——只渲染
    /// 最外层那句话的话，"打不开数据库"后面那个真正有用的"文件权限不足"就没了。
    #[must_use]
    pub(crate) fn startup_failed(kind: &'static str, error: &dyn std::error::Error) -> Self {
        let mut message = error.to_string();
        let mut source = error.source();
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        Self {
            outcome: Outcome::StartupFailed(StartupFailure { kind, message }),
        }
    }

    /// 跑起来又停下来了。
    #[must_use]
    pub(crate) fn stopped(
        cause: Option<StopCause>,
        exits: Vec<TaskExit>,
        unreaped: Vec<UnreapedTask>,
        forced: bool,
        plan_invalid: bool,
        storage_close: CloseOutcome,
    ) -> Self {
        Self {
            outcome: Outcome::Stopped(Stopped {
                cause,
                exits,
                unreaped,
                forced,
                plan_invalid,
                storage_close,
            }),
        }
    }

    /// 只打印了点东西就走了（`--help` / `--version`）。
    #[must_use]
    pub(crate) const fn printed(action: &'static str) -> Self {
        Self {
            outcome: Outcome::Printed { action },
        }
    }

    /// 这一次如果只是打印，打印的是什么；否则 `None`。
    ///
    /// 给用例用的。`run` 的调用方（`main`）只需要退出码，不需要问这个。
    #[must_use]
    pub fn printed_action(&self) -> Option<&'static str> {
        match &self.outcome {
            Outcome::Printed { action } => Some(action),
            Outcome::StartupFailed(_) | Outcome::Stopped(_) => None,
        }
    }

    /// 这次运行算不算干净收尾。
    ///
    /// 启动失败直接是 `false`：一次没跑起来的运行谈不上"停得干净"。
    /// `--help` / `--version` 是 `true`：它们做到了自己要做的全部事情。
    #[must_use]
    pub fn succeeded(&self) -> bool {
        match &self.outcome {
            Outcome::StartupFailed(_) => false,
            Outcome::Stopped(stopped) => stopped.succeeded(),
            Outcome::Printed { .. } => true,
        }
    }

    /// 启动失败的详情。跑起来过就是 `None`。
    #[must_use]
    pub fn startup_failure(&self) -> Option<&StartupFailure> {
        match &self.outcome {
            Outcome::StartupFailed(failure) => Some(failure),
            Outcome::Stopped(_) => None,
            Outcome::Printed { .. } => None,
        }
    }

    /// 这次关停的触发源。
    #[must_use]
    pub fn cause(&self) -> Option<&StopCause> {
        match &self.outcome {
            Outcome::StartupFailed(_) => None,
            Outcome::Stopped(stopped) => stopped.cause.as_ref(),
            Outcome::Printed { .. } => None,
        }
    }

    /// 全部任务退出记录。
    #[must_use]
    pub fn exits(&self) -> &[TaskExit] {
        match &self.outcome {
            Outcome::StartupFailed(_) => &[],
            Outcome::Stopped(stopped) => &stopped.exits,
            Outcome::Printed { .. } => &[],
        }
    }

    /// 到点仍未回收的任务。
    #[must_use]
    pub fn unreaped(&self) -> &[UnreapedTask] {
        match &self.outcome {
            Outcome::StartupFailed(_) => &[],
            Outcome::Stopped(stopped) => &stopped.unreaped,
            Outcome::Printed { .. } => &[],
        }
    }

    /// 关池的结论。启动失败时没有池，因此是 `None`。
    #[must_use]
    pub fn storage_close(&self) -> Option<&CloseOutcome> {
        match &self.outcome {
            Outcome::StartupFailed(_) => None,
            Outcome::Stopped(stopped) => Some(&stopped.storage_close),
            Outcome::Printed { .. } => None,
        }
    }

    /// 关停过程中是否动用过 `abort()`。
    #[must_use]
    pub fn forced(&self) -> bool {
        match &self.outcome {
            Outcome::StartupFailed(_) => false,
            Outcome::Stopped(stopped) => stopped.forced,
            Outcome::Printed { .. } => false,
        }
    }

    /// 预算计划是否无效（时长溢出，fail-closed）。
    #[must_use]
    pub fn cleanup_failed(&self) -> bool {
        match &self.outcome {
            Outcome::StartupFailed(_) => false,
            Outcome::Stopped(stopped) => stopped.plan_invalid,
            Outcome::Printed { .. } => false,
        }
    }
}

/// 把报告翻成进程退出码。
///
/// 只有两个取值。**不给每一类失败分配一个专属数字**：退出码是编排系统唯一能看到的东西，
/// 而编排系统只会问一个问题——"要不要重启它"。把失败细分成 2/3/4/5，第一个后果是有人
/// 开始在重启策略里写 `restart_on: [3, 5]`，第二个后果是我们再也不能改这些数字。
/// 详情该去日志里读，那里有结构化事件；退出码只回答那个是非题。
///
/// 另一条边界：`ExitCode` 在 POSIX 上只有低 8 位有效，`ExitCode::from(256)` 会变成 0。
/// 两个取值从根上避开这个坑。
#[must_use]
pub fn exit_code(report: &RunReport) -> ExitCode {
    if report.succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use service_core::shutdown::Stage;
    use service_core::storage::StorageError;
    use service_core::task::{RuntimeId, ShutdownClass, TaskName, TaskSpec, TaskSupervisor};
    use tokio::runtime::Handle;

    use crate::signals::ProcessSignal;

    fn returned(name: TaskName) -> TaskExit {
        TaskExit {
            name: Some(name),
            runtime: None,
            kind: ExitKind::Returned,
        }
    }

    /// 一次完美的关停。每个用例从它出发，只改一项。
    fn clean() -> RunReport {
        RunReport::stopped(
            Some(StopCause::Signal(ProcessSignal::Terminate)),
            vec![returned(TaskName::Http), returned(TaskName::Ticker)],
            Vec::new(),
            false,
            false,
            CloseOutcome::Closed,
        )
    }

    #[test]
    fn a_clean_shutdown_succeeds() {
        let report = clean();
        assert!(report.succeeded());
        assert_eq!(exit_code(&report), ExitCode::SUCCESS);
    }

    #[test]
    fn each_of_the_six_conjuncts_can_fail_on_its_own() {
        // 这条用例守的就是"六项合取"本身：任何一项被人悄悄删掉，这里都会红。
        // 每个 case 都只破坏一项，其余五项保持干净。
        let unreaped = UnreapedTask {
            name: Some(TaskName::Ticker),
            runtime: RuntimeId::of(
                &tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
                    .handle()
                    .clone(),
            ),
            class: ShutdownClass::Abortable,
            stalled_at: Stage::Reap,
        };

        let cases: Vec<(&str, RunReport)> = vec![
            (
                "① 触发源不是信号",
                RunReport::stopped(
                    Some(StopCause::SupervisorDrained),
                    vec![returned(TaskName::Http)],
                    Vec::new(),
                    false,
                    false,
                    CloseOutcome::Closed,
                ),
            ),
            (
                "② 动用过 abort",
                RunReport::stopped(
                    Some(StopCause::Signal(ProcessSignal::Terminate)),
                    vec![returned(TaskName::Http)],
                    Vec::new(),
                    true,
                    false,
                    CloseOutcome::Closed,
                ),
            ),
            (
                "③ 预算计划无效",
                RunReport::stopped(
                    Some(StopCause::Signal(ProcessSignal::Terminate)),
                    vec![returned(TaskName::Http)],
                    Vec::new(),
                    false,
                    true,
                    CloseOutcome::Closed,
                ),
            ),
            (
                "④ 有任务没收回来",
                RunReport::stopped(
                    Some(StopCause::Signal(ProcessSignal::Terminate)),
                    vec![returned(TaskName::Http)],
                    vec![unreaped],
                    false,
                    false,
                    CloseOutcome::Closed,
                ),
            ),
            (
                "⑤ 存储没关干净",
                RunReport::stopped(
                    Some(StopCause::Signal(ProcessSignal::Terminate)),
                    vec![returned(TaskName::Http)],
                    Vec::new(),
                    false,
                    false,
                    CloseOutcome::Failed(StorageError::Internal {
                        context: Box::from("pool close failed"),
                    }),
                ),
            ),
            (
                "⑥ 有任务不是正常返回",
                RunReport::stopped(
                    Some(StopCause::Signal(ProcessSignal::Terminate)),
                    vec![
                        returned(TaskName::Http),
                        TaskExit {
                            name: Some(TaskName::Ticker),
                            runtime: None,
                            kind: ExitKind::Cancelled,
                        },
                    ],
                    Vec::new(),
                    false,
                    false,
                    CloseOutcome::Closed,
                ),
            ),
        ];

        assert_eq!(cases.len(), 6, "六项合取，就该有六个反例");
        for (label, report) in cases {
            assert!(!report.succeeded(), "{label}：这一项不成立时不该报成功");
            assert_eq!(exit_code(&report), ExitCode::FAILURE, "{label}");
        }
    }

    #[test]
    fn a_skipped_pool_close_is_not_a_success() {
        // "举不出证据所以没关" 既不是成功也不是失败——但它**不能**算干净收尾，
        // 否则 SkippedUnproven 这个变体存在的意义就没了。
        let report = RunReport::stopped(
            Some(StopCause::Signal(ProcessSignal::Terminate)),
            vec![returned(TaskName::Http)],
            Vec::new(),
            false,
            false,
            CloseOutcome::SkippedUnproven {
                reason: "a plane may still hold the pool",
            },
        );
        assert!(!report.succeeded());
    }

    #[test]
    fn a_startup_failure_never_succeeds_and_keeps_the_source_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("cannot open the database")]
        struct Outer(#[source] Inner);
        #[derive(Debug, thiserror::Error)]
        #[error("permission denied")]
        struct Inner;

        let report = RunReport::startup_failed("storage", &Outer(Inner));
        assert!(!report.succeeded());
        assert_eq!(exit_code(&report), ExitCode::FAILURE);

        let failure = report.startup_failure().expect("启动失败必须带详情");
        assert_eq!(failure.kind(), "storage");
        assert_eq!(
            failure.message(),
            "cannot open the database: permission denied",
            "只渲染最外层那句话的话，真正有用的原因就丢了"
        );
        assert!(report.cause().is_none());
        assert!(report.exits().is_empty());
        assert!(report.storage_close().is_none());
    }

    // 这条用例**必须**跑在多线程调度器上，理由与 `core` 里
    // `reap_gives_up_at_its_own_deadline` 完全相同：它用一个同步阻塞的任务来制造
    // "abort 掐不动"的局面，而在 current-thread 上那次阻塞会占住唯一的工作线程，
    // 连 `reap` 自己都排不上队——等它睡完，任务已经正常退出，用例于是测了个反面。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forced_shutdown_reports_failure_not_success() {
        // 「如实上报」的落点，也是一条回归用例：要是「干净停下来」和「掐了还没掐掉」
        // 最后都是退出码 0、`info!("runtime stopped")` 超时也照喊，那"关停成功了没"
        // 这个问题在进程外面就没有答案。
        //
        // 这条用例**不手搓** `HarvestOutcome`：它让一个真的掐不掉的任务跑一遍
        // `TaskSupervisor::reap`，再把收割的结论**原样**交给 `RunReport::stopped`。
        // 手搓一份结论会把两端之间那段翻译绕过去，而那正是会接错的地方——
        // 验的是「收割说的话」与「退出码说的话」是不是同一句。
        let mut supervisor = TaskSupervisor::new();
        supervisor
            .spawn_on(
                TaskSpec::new(TaskName::Ticker, ShutdownClass::Abortable),
                &Handle::current(),
                Box::pin(async {
                    // 同步阻塞：`abort()` 只能取消停在 `.await` 上的任务，对它无效。
                    // 睡 1s 是为了活过下面那个 200ms 的 deadline，同时不让 runtime
                    // 在 drop 时为这个工作线程多等几秒。
                    std::thread::sleep(Duration::from_secs(1));
                    Ok(())
                }),
            )
            .expect("第一次登记不该重名");

        // 让任务真正跑起来，否则 abort 会赶在它开始之前生效，这一次关停就是干净的。
        tokio::time::sleep(Duration::from_millis(50)).await;
        let outcome = supervisor
            .reap(Instant::now() + Duration::from_millis(200))
            .await;

        // 前提：这一次收割确实是强杀。这条断言不是在验被测代码，是在验用例自己
        // 造出了它想造的局面——它红了说明用例失效了，不说明报告错了。
        assert!(outcome.forced, "用例没能造出一次强杀：{outcome:?}");
        assert!(!outcome.unreaped.is_empty());

        let report = RunReport::stopped(
            Some(StopCause::Signal(ProcessSignal::Terminate)),
            outcome.exits,
            outcome.unreaped,
            outcome.forced,
            false,
            CloseOutcome::Closed,
        );

        // 结论行必须反映真实结果：触发源是一次正常的信号、存储也关干净了，
        // 但有任务是被掐掉的——这一次关停就不是成功。
        assert!(
            !report.succeeded(),
            "掐不掉的任务被报成了一次干净关停：{report:?}"
        );
        assert_eq!(
            exit_code(&report),
            ExitCode::FAILURE,
            "报告说失败，退出码却说成功——进程外面就看不出区别了"
        );
        assert!(report.forced(), "强杀这件事必须能被单独问出来");
        assert!(
            !report.unreaped().is_empty(),
            "「哪些没收回来」不能在翻译成报告时丢掉——只剩一个布尔就没法定位了"
        );
        assert!(!report.cleanup_failed(), "预算计划本身是好的，不该记成失败");
    }

    #[test]
    fn a_shutdown_with_no_tasks_at_all_still_reports_its_cause() {
        // 空 supervisor 留痕后正常退出——但不算成功。
        let report = RunReport::stopped(
            Some(StopCause::SupervisorDrained),
            Vec::new(),
            Vec::new(),
            false,
            false,
            CloseOutcome::Closed,
        );
        assert!(!report.succeeded(), "一个面都没有不是一次正常运行");
        assert_eq!(
            report.cause().map(StopCause::as_str),
            Some("supervisor_drained")
        );
    }
}
