//! 一份读不懂的配置：进程必须**起不来**，而且要说清是哪儿读不懂。
//!
//! 一个文件一条用例，理由见 `tests/run.rs` 的模块文档（`run` 每进程只能调一次）。

#![allow(unused_crate_dependencies)]

use std::ffi::OsString;
use std::pin::Pin;
use std::process::ExitCode;
use std::task::{Context, Poll};

use futures_core::Stream;
use service_app::{ProcessEnv, ProcessSignal, exit_code, run};
use service_testkit::TempInstallRoot;

/// 与 `tests/run.rs` 同一份夹具。这一条用例走不到主循环，所以它是空的。
struct Canned(std::vec::IntoIter<ProcessSignal>);

impl Stream for Canned {
    type Item = ProcessSignal;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.next())
    }
}

/// 一个带**换行**的未知键。
///
/// 两件事一起验：
///
/// 1. 未知键是错误，不是"忽略掉继续跑"（拼错一个键就得到一个静默按默认值跑的服务，
///    是本模板明确拒绝的一种行为）。
/// 2. 键名来自文件内容，也就是用户输入。它原样进日志的话，这一行里的 `\n` 就能在
///    日志流里**伪造出一条记录**——下面那行 `level=INFO ... everything is fine` 正是
///    伪造出来的样子。`FieldPath` / `SafeMessage` 的消毒把控制字符剥掉，这条用例是
///    那件事唯一的端到端证据。
const INJECTING_CONFIG: &str = "\
[http]
\"bind_addr\\nlevel=INFO msg=everything-is-fine unknown\" = 1
";

#[test]
fn an_unreadable_config_fails_startup_and_never_forges_a_log_line() {
    let root = TempInstallRoot::new().expect("临时安装根");
    root.write_config(INJECTING_CONFIG).expect("写测试配置");

    let env = ProcessEnv {
        exe_path: root.exe_path().to_path_buf(),
        args: vec![OsString::from("service")],
        vars: std::collections::BTreeMap::new(),
        stderr_is_terminal: false,
        bin_name: "service",
    };

    let report = run(Ok(env), || Ok(Canned(Vec::new().into_iter())));

    // ① 起不来，而且退出码说得清。
    let failure = report
        .startup_failure()
        .unwrap_or_else(|| panic!("这份配置不该能起来：{report:?}"));
    assert_eq!(
        failure.kind(),
        "config",
        "失败该被归到配置这一类，而不是别的：{failure:?}"
    );
    assert!(!report.succeeded());
    assert_eq!(exit_code(&report), ExitCode::FAILURE);

    // ② 说清了是哪儿读不懂——原样带上了那个键名的可见部分。
    let message = failure.message();
    assert!(
        message.contains("unknown"),
        "错误该指出是哪个键读不懂：{message}"
    );

    // ③ **但**控制字符一个都不剩。少了这一条，上面那个键名会在日志里多长出一行
    //    `level=INFO msg=everything-is-fine`。
    assert!(
        !message.chars().any(char::is_control),
        "错误信息里还留着控制字符，日志可以被伪造：{message:?}"
    );

    // ④ 还没到装配那一步：数据库文件不该被创建出来。配置读不懂就先建库，
    //    代价是一份"启动失败却留下了副作用"的安装。
    assert!(
        !root.data_dir().join("service.sqlite3").exists(),
        "配置都没读懂，不该已经开过库了"
    );
}
