//! 整条 [`run`] 路径：进程事实进去，一份 [`RunReport`] 出来。
//!
//! # 一个测试二进制里只能有一次 `run`
//!
//! [`run`] 会调用 `tracing::subscriber::set_global_default`，而那个槽位每个进程只有一个。
//! 第二次调用拿到的是 `AlreadyInstalled`，`run` 会把它变成一次 `telemetry` 启动失败——
//! 于是第二条用例验的不再是它想验的东西，而且**它会不会红取决于测试的执行顺序**。
//!
//! 同一条约束还有另一半：`service_testkit::LogCapture` 装的也是那个槽位。所以
//! 「调 `run`」与「断言日志」在一个二进制里只能有一个。本模板的分法是：
//!
//! - 要读日志的编排层用例 → `app` 的**单元测试**（`lifecycle::r#loop` 与 lib 根部的
//!   `tests`），那里不经过 `run`，`LogCapture` 能正常工作；
//! - 要跑完整条 `run` 的用例 → `app/tests/` 下**一个文件一条**，断言 [`RunReport`]、
//!   退出码与文件系统效果。
//!
//! [`run`]: service_app::run
//! [`RunReport`]: service_app::RunReport

// 这个 target 只用到 `service_app` 与三个夹具 crate；`app` 声明的其余十几条依赖服务于
// lib target，在那边照常被 lint 管着。理由同 `src/main.rs` 顶上那条。
#![allow(unused_crate_dependencies)]

use std::ffi::OsString;
use std::pin::Pin;
use std::process::ExitCode;
use std::task::{Context, Poll};

use futures_core::Stream;
use service_app::{ProcessEnv, ProcessSignal, exit_code, run};
use service_core::task::{ExitKind, TaskName};
use service_testkit::TempInstallRoot;

/// 一条把事先写好的信号依次吐出来、然后结束的流。
///
/// 这就是「不靠起进程来验收」的那个注入点：生产上 `main` 传的是
/// `os_signal_stream`，这里传的是一个产生这条流的闭包。被测代码一个字都不用改。
struct Canned(std::vec::IntoIter<ProcessSignal>);

impl Stream for Canned {
    type Item = ProcessSignal;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.next())
    }
}

/// 一份能真的跑起来的配置。
///
/// 只覆盖两个键，其余全部走内置默认值：
///
/// - `bind_addr` 的端口取 **0**，让内核挑。写死 8080 的话，这条用例会在"开发机上恰好有
///   别的东西占着 8080"时红，而那与被测代码无关；并行跑两个这样的二进制也会互相打架。
/// - `filter` 取 `warn`：断言全在返回值上，`info` 那一串会淹掉 `cargo test` 的输出。
///   不取 `off` 是因为用例红的时候，那几条 `warn`/`error` 正是唯一的线索——
///   `run` 装的 subscriber 直接写 stderr，libtest 捕获不到，关掉就真的什么都没有了。
const TEST_CONFIG: &str = "\
[http]
bind_addr = \"127.0.0.1:0\"

[telemetry]
filter = \"warn\"
";

#[test]
fn a_synthetic_stop_runs_the_whole_process_and_reports_a_clean_shutdown() {
    // 跨过 `run` 才验得到的那几条。顺序证据在 `lifecycle::r#loop` 的单元
    // 测试里（见模块文档），这里验的是**结论**：一次正常的起停，报告必须说它成功了，
    // 而退出码必须跟着报告走。
    //
    // 这条用例同时是「Ctrl-C 之后每个任务都有闭合记录」在进程内的对应物：
    // `report.exits()` 里必须有全部在册的面，而且每一个都是自己返回的，不是被掐掉的。
    let root = TempInstallRoot::new().expect("临时安装根");
    root.write_config(TEST_CONFIG).expect("写测试配置");

    let env = ProcessEnv {
        exe_path: root.exe_path().to_path_buf(),
        args: vec![OsString::from("service")],
        // 空环境变量表：`<PREFIX>_CONFIG` 一旦存在就会抢在缺省位置前面，而上面那份配置
        // 正是写在缺省位置上的。
        vars: std::collections::BTreeMap::new(),
        stderr_is_terminal: false,
        bin_name: "service",
    };

    let report = run(Ok(env), || {
        Ok(Canned(vec![ProcessSignal::Terminate].into_iter()))
    });

    assert!(
        report.startup_failure().is_none(),
        "这份配置该能起来：{report:?}"
    );
    assert!(report.succeeded(), "一次正常的起停该是成功：{report:?}");
    assert_eq!(exit_code(&report), ExitCode::SUCCESS);

    // 没有掐不掉的任务，也没有走强制退出那条路。
    assert!(
        report.unreaped().is_empty(),
        "干净关停不该留下掐不掉的任务：{:?}",
        report.unreaped()
    );
    assert!(!report.forced(), "干净关停不该被记成强制退出");
    assert!(!report.cleanup_failed(), "收尾没失败，却被记成失败了");

    // 每个在册的面都有一条闭合记录，且都是**自己返回**的。
    let mut names: Vec<TaskName> = report
        .exits()
        .iter()
        .map(|exit| {
            assert!(
                matches!(exit.kind, ExitKind::Returned),
                "{:?} 不是自己返回的：{:?}",
                exit.name,
                exit.kind
            );
            exit.name.expect("在册的面都有名字")
        })
        .collect();
    names.sort_unstable_by_key(|name| name.as_str());
    assert_eq!(
        names,
        vec![TaskName::Http, TaskName::Ticker],
        "默认配置有 http 与 ticker 两个面，两个都该留下闭合记录"
    );
}
