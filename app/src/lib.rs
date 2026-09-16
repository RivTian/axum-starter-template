//! 装配层：把进程环境变成一个跑起来的服务，再把它干净地停下来。
//!
//! # 这一层只有一个入口
//!
//! [`run`] 收下一份**进程事实**（[`ProcessEnv`]）和一条**信号流**，还回一份
//! [`RunReport`]。它不读 `std::env`、不装 `ctrl_c`、不 `exit`——那三件事全在参数和返回值
//! 里。代价是 `main.rs` 要多写一行；收益是整条启动—运行—关停路径可以在一个用例里跑完，
//! 而用例既不需要真起一个进程，也不需要真发一个信号。
//!
//! # 启动的固定顺序
//!
//! ```text
//! ① 进程事实         ProcessEnv       ← 由 main 抄下来传进来
//! ② 命令行           cli::parse       ← --help / --version 在这里就走
//! ③ 安装根           paths::install_root_from
//! ④ 配置             bootstrap::load  ← 三段优先级；文件缺失的处理见该模块文档
//! ⑤ subscriber       telemetry::init  ← **配置之后**，所以 ①～④ 的失败没有结构化日志
//! ⑥ runtime          RuntimeSet::build
//! ── 进入 block_on ──
//! ⑦ 信号             stops()          ← 必须在 runtime 上下文里，且**早于**装配
//! ⑧ 装配 + 起任务    assemble → launch
//! ⑨ 编排             r#loop::orchestrate
//! ── block_on 返回 ──
//! ⑩ 关 runtime       RuntimeSet::shutdown  ← 必须在 block_on 返回之后（在 runtime 里关它会 panic）
//! ```
//!
//! ⑤ 排在 ④ 之后是有代价的：过滤器、格式、着色全写在配置文件里，而在读到那个文件之前，
//! 一条日志都发不出去。这里的处理是 [`fail_before_telemetry`]——用**默认**遥测配置装一个
//! subscriber，专门把这条失败发出去，然后立刻返回。于是「没有结构化日志」缩小成
//! 「这条日志用的是默认格式」，而不是「这条日志不存在」。
//!
//! 反过来做——先用默认配置装 subscriber，读完配置再换一个——要么需要一个可重装的全局，
//! 要么需要 `reload::Handle`。前者是本模板不留的那种全局可变状态，后者会让"日志格式"成为
//! 唯一一个能在运行期被换掉却不走配置热重载那条路的东西。
//!
//! # ⑦ 为什么在这个位置，以及为什么它是个函数
//!
//! 两条实测把这一步钉死在这里：
//!
//! 1. `tokio::signal::unix::signal()` 在 runtime 之外调用会 **panic**（`there is no reactor
//!    running`）。所以它不可能由 `main` 产生后传进来——`main` 手上没有 runtime。`run` 收
//!    的因此是 `FnOnce() -> io::Result<S>`，在 `block_on` 里面调用。
//! 2. 注册之后、第一次 poll 之前到达的信号**不会丢**（tokio 1.x 实测：`recv()` 立刻返回
//!    `Some`）。所以把注册排在装配**之前**是有意义的：装配可能很慢（建连接池、绑端口），
//!    而"默认处置仍然生效"的那段窗口里收到的 SIGTERM 会直接杀掉进程、跳过全部收尾。
//!    注册提前之后，那段窗口只剩 ①～⑥，且这几步都不做 I/O 等待。
//!
//! 装配期间到达的停止请求不会被立刻处理——提交门在等各面就绪。它会在提交门通过后的第一次
//! `select!` 上被读到，于是进程走完整条关停序列。这比"被 SIGKILL"好，但不是零延迟；真正
//! 的「装配途中放弃」是 [`boot::gate`] 的事，那条路由面自己的失败触发。

mod boot;
mod cli;
mod config;
mod lifecycle;
mod rt;
mod signals;
mod telemetry;

use std::io::{self, IsTerminal};
use std::path::Path;
use std::time::Instant;

use futures_core::Stream;
use service_core::build_info::BuildInfo;
use service_core::config::TelemetryConfig;
use service_core::paths;
use tracing::{error, info, warn};

use boot::assembly::{self, BootError};
use config::bootstrap::{self, TemplateAction};
use config::reload::ReloadSource;
use lifecycle::r#loop::{self, Finished};
use rt::RuntimeSet;
use telemetry::FilterOutcome;

pub use boot::ProcessEnv;
pub use lifecycle::report::{RunReport, StartupFailure, exit_code};
pub use lifecycle::stop::StopCause;
pub use signals::{ProcessSignal, os_signal_stream};

/// 跑完一次运行。
///
/// `env` 是 [`ProcessEnv::capture`] 的结果，**连同它的失败一起**收下：`current_exe()` 拿不到
/// 时安装根就无从谈起，而那条失败和其他启动失败没有本质区别，不该逼着 `main` 去分支处理
/// （`main` 是一个无分支表达式）。
///
/// `stops` 是停止与重载请求的**产生器**。生产上直接传函数项 [`os_signal_stream`]，用例里
/// 传 `|| Ok(一条写死的流)`。它是函数而不是流，因为信号处理器只能在 runtime 上下文里注册
/// （见模块文档 ⑦）；注入点没有变，变的只是注入的是值还是产生值的函数。
///
/// 这个函数**不 panic、不 `exit`、不返回 `Result`**：一次运行的所有结局都是
/// [`RunReport`] 的一种形态，退出码由 [`exit_code`] 从它算出来。
#[must_use]
pub fn run<F, S>(env: io::Result<ProcessEnv>, stops: F) -> RunReport
where
    F: FnOnce() -> io::Result<S>,
    S: Stream<Item = ProcessSignal>,
{
    // ① 进程事实。
    let env = match env {
        Ok(env) => env,
        Err(err) => return fail_before_telemetry("environment", &err, io::stderr().is_terminal()),
    };

    // ② 命令行。
    let cli = match cli::parse(&env.args) {
        Ok(cli) => cli,
        Err(err) => return fail_before_telemetry("cli", &err, env.stderr_is_terminal),
    };

    let build = BuildInfo::current(env.bin_name);
    match cli.action {
        // `--help` / `--version` 走 stdout，而且**在 subscriber 之前**：它们是程序的输出，
        // 不是日志。`prog --version | cat` 因此拿到的是干净的一行。
        cli::Action::Help => {
            println!("{}", cli::usage(build.service));
            return RunReport::printed("help");
        }
        cli::Action::Version => {
            println!("{} {}", build.service, build.version);
            return RunReport::printed("version");
        }
        cli::Action::Run => {}
    }

    // ③ 安装根：可执行文件**所在**的那个目录。全进程只在这里算一次。
    let install_root = paths::install_root_from(&env.exe_path);

    // ④ 配置。
    let loaded = match bootstrap::load(cli.config.as_deref(), &env, &install_root) {
        Ok(loaded) => loaded,
        Err(err) => return fail_before_telemetry("config", &err, env.stderr_is_terminal),
    };

    // ⑤ subscriber。从这一行往下才有结构化日志。
    let filter = match telemetry::init(&loaded.config.telemetry, env.stderr_is_terminal) {
        Ok(outcome) => outcome,
        Err(err) => return fail_before_telemetry("telemetry", &err, env.stderr_is_terminal),
    };

    // ④ 与 ⑤ 攒下来的那几件事，现在才有地方记。顺序是"先说日志本身有没有问题，
    // 再说配置有没有问题"——一条被退回的过滤器会影响下面每一条日志能不能被看见。
    replay(&filter, &loaded, build, &install_root);

    // 关停预算与 runtime 拓扑都是冷段，在配置被交给 `ConfigPublisher` 之前读走。
    // `Budgets` 是 `Copy`，这里拿的是一份独立的值，不是对那份配置的借用。
    let budgets = loaded.config.shutdown;

    // ⑥ runtime。
    let runtimes = match RuntimeSet::build(&loaded.config.runtime) {
        Ok(runtimes) => runtimes,
        Err(err) => {
            error!(name: "startup_failed", kind = "runtime", error = %err, "startup failed");
            return RunReport::startup_failed("runtime", &err);
        }
    };
    info!(
        name: "runtimes_built",
        count = runtimes.len(),
        "tokio runtimes built"
    );

    // 重载要读的文件与展开用的环境变量，在这里定下来，运行期只读（见 `config::reload`）。
    let reload = ReloadSource::new(loaded.path, loaded.source, &env, install_root);

    // ⑦⑧⑨ 注册信号、装配、起任务、编排。全部在主 runtime 上跑到完。
    let executors = runtimes.executors();
    let outcome = runtimes.block_on(async {
        // ⑦ 先注册再装配。`io::Error` 不是本 workspace 的类型，所以这里显式 `map_err`
        // 而不是 `#[from]`。
        let stops = stops().map_err(StartFailure::Signals)?;
        // ⑧
        let assembled = assembly::assemble(loaded.config, build, &executors).await?;
        Ok::<Finished, StartFailure>(r#loop::orchestrate(assembled.launch(), &reload, stops).await)
    });

    // ⑩ 关 runtime。**必须在 `block_on` 返回之后**：在一个 runtime 的上下文里关这个
    // runtime 会 panic。启动失败那条路上没有 `Plan`，就地给它一份同样的预算。
    let (report, deadline) = match outcome {
        Ok(finished) => (finished.report, finished.runtime_deadline),
        Err(err) => {
            error!(
                name: "startup_failed",
                kind = err.kind(),
                error = %err,
                "startup failed inside the runtime"
            );
            let report = RunReport::startup_failed(err.kind(), &err);
            (report, Instant::now() + budgets.runtime_shutdown)
        }
    };

    for record in runtimes.shutdown(deadline) {
        info!(
            name: "runtime_stopped",
            runtime = %record.name,
            budget_ms = %record.budget.as_millis(),
            elapsed_ms = %record.elapsed.as_millis(),
            "runtime stopped"
        );
    }

    report
}

/// `block_on` 里面那两步各自的失败。
///
/// 存在的唯一理由是 `?`：⑦ 与 ⑧ 失败的类型不同，而它们之后的处理完全一样（记一条
/// `startup_failed`、按同一份预算关 runtime）。把它们并成一个类型，而不是写两层
/// `match`，是为了让那段处理只写一遍——两份「启动失败」的处理迟早会分叉。
///
/// 它不外露：调用方看到的只有 [`RunReport`]。
#[derive(Debug, thiserror::Error)]
enum StartFailure {
    /// ⑦ 装不上信号处理器。**不兜底**，见 `signals::os_signal_stream` 的文档。
    #[error("cannot install signal handlers: {0}")]
    Signals(io::Error),
    /// ⑧ 装配失败。
    #[error(transparent)]
    Boot(#[from] BootError),
}

impl StartFailure {
    /// 日志与报告里用的稳定短名。
    ///
    /// 装配那一支透传 [`BootError::kind`]，因为运维要区分的是"连不上库"还是"端口被占"，
    /// 而不是"失败发生在 lib.rs 还是 assembly.rs"。
    fn kind(&self) -> &'static str {
        match self {
            Self::Signals(_) => "signals",
            Self::Boot(err) => err.kind(),
        }
    }
}

/// 把 ①～⑤ 里攒下来的事实补记成日志。
///
/// 这些事在发生的那一刻还没有 subscriber，各自的产生方一律"把事实作为数据交出来"
/// 而不是就地记日志。补记的地方只有这一处——同一件事记两遍会让运维以为发生了两次。
fn replay(
    filter: &FilterOutcome,
    loaded: &bootstrap::Loaded,
    build: BuildInfo,
    install_root: &Path,
) {
    info!(
        name: "service_starting",
        service = build.service,
        version = build.version,
        "service starting"
    );

    if let FilterOutcome::FellBack { requested, reason } = filter {
        warn!(
            name: "telemetry_filter_fell_back",
            requested = %requested.as_str(),
            reason = %reason,
            "the configured tracing filter could not be parsed; using the built-in default"
        );
    }

    // `install_root` 记在这一条上，而不是单独一条。它和 `path` 是同一个问题的两半：
    // "配置从哪来"以及"数据会去哪"，两者都锚在这个目录上。分成两条记，运维要
    // 在日志里对齐两个时间戳才能拼出一次启动的落脚点；而漏掉它的后果是最难查的那一类——
    // 进程起来了、日志干净、写的却是另一个目录下的库。
    info!(
        name: "config_loaded",
        path = %loaded.path.display(),
        selected_by = loaded.source.as_str(),
        install_root = %install_root.display(),
        "config loaded"
    );

    match &loaded.template {
        TemplateAction::Untouched => {}
        TemplateAction::Written => info!(
            name: "config_template_written",
            path = %loaded.path.display(),
            "no config file was present; wrote the built-in template"
        ),
        TemplateAction::WriteFailed { reason } => warn!(
            name: "config_template_write_failed",
            path = %loaded.path.display(),
            reason = %reason,
            "could not write the built-in template; continuing with the in-memory defaults"
        ),
    }

    // 钳位逐条记。它们是"你写的值不合法，我替你改了"，每一条都该能被单独看见——
    // 重载路径上的钳位并进那一条重载报告，两边不重复（见 `config::reload`）。
    for clamp in &loaded.clamps {
        warn!(
            name: "config_clamped",
            field = clamp.field.as_str(),
            reason = clamp.reason,
            "a configured value was out of range and has been clamped"
        );
    }
}

/// subscriber 还没装好就失败了：用**默认**遥测配置临时装一个，把这条失败发出去。
///
/// 只在返回前调用一次，所以不会和 ⑤ 的那次 `init` 撞车——两条路径互斥。装不上时（真正的
/// "已经装过了"）就只剩下返回值本身，退出码仍然是对的。
fn fail_before_telemetry(
    kind: &'static str,
    error: &dyn std::error::Error,
    stderr_is_terminal: bool,
) -> RunReport {
    let _ = telemetry::init(&TelemetryConfig::default(), stderr_is_terminal);
    error!(
        name: "startup_failed",
        kind,
        error = %error,
        "startup failed before telemetry was configured"
    );
    RunReport::startup_failed(kind, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;

    use service_testkit::{LogCapture, TempInstallRoot};

    /// 一个锚在给定假可执行文件上的 [`ProcessEnv`]。
    ///
    /// 环境变量表是**空的**，这不是图省事：`<PREFIX>_CONFIG` 一旦存在就会抢在缺省位置
    /// 前面（三段优先级），而这几条用例要验的恰恰是最后那一档。空表让"缺省位置"
    /// 成为唯一可能的来源，不依赖跑用例的那台机器上碰巧没设什么变量。
    fn env_at(root: &TempInstallRoot) -> ProcessEnv {
        ProcessEnv {
            exe_path: root.exe_path().to_path_buf(),
            args: vec![OsString::from("service")],
            vars: std::collections::BTreeMap::new(),
            stderr_is_terminal: false,
            bin_name: "service",
        }
    }

    #[test]
    fn a_different_executable_moves_config_data_and_the_logged_root_together() {
        // 「全进程只有一个路径锚点」在编排层的落点。
        //
        // `core::paths` 那几条用例已经证明了纯函数层面的锚定；这里要证明的是**接线**：
        // `ProcessEnv.exe_path` 真的一路走到了三个地方——配置文件去哪儿找、数据库落在
        // 哪儿、`config_loaded` 报的那个 `install_root` 是什么。三者中任何一条接错了，
        // 症状都是同一个："进程起来了、日志干净、写的却是另一个目录下的库"。
        //
        // 用三个安装根而不是一个：一个根验不出"跟着锚点走"，只验得出"等于某个常量"。
        // 一处把 cwd 当锚点的代码，在单根用例下会照常通过。
        let roots: Vec<TempInstallRoot> = (0..3)
            .map(|_| TempInstallRoot::new().expect("临时安装根"))
            .collect();

        let capture = LogCapture::start().expect("装捕获用的 subscriber");
        for (i, root) in roots.iter().enumerate() {
            let env = env_at(root);

            // 与 `run` 的第③步**同一个调用**。这里不另抄一份推导——抄一份的话，
            // 这条用例验的就是那份抄件，而不是生产路径。
            let install_root = paths::install_root_from(&env.exe_path);
            assert_eq!(
                install_root,
                root.root(),
                "TC{i}：锚点该是可执行文件所在目录"
            );

            let loaded = bootstrap::load(None, &env, &install_root)
                .unwrap_or_else(|err| panic!("TC{i}：首次启动该落下模板并读回来：{err}"));

            // ① 配置文件去哪儿找：`<锚点>/config/service.toml`，而且真的被创建了出来。
            assert_eq!(loaded.path, root.default_config_path(), "TC{i}：配置路径");
            assert!(loaded.path.exists(), "TC{i}：缺省位置首启时该落下模板");

            // ② 数据库落在哪儿：模板里写的是相对路径，`finalize` 把它锚到同一个根上。
            //    判据用 `starts_with(data_dir)` 而不是整串相等——文件名是配置内容，
            //    改它不该让这条用例红；跑到别的根下面才该红。
            assert!(
                loaded.config.storage.path.starts_with(root.data_dir()),
                "TC{i}：数据库跑到锚点外面去了：{}",
                loaded.config.storage.path.display()
            );

            // ③ 日志里报的那个根。`replay` 是 `run` 里唯一记这条的地方，直接调它。
            replay(
                &FilterOutcome::Applied,
                &loaded,
                BuildInfo::current("test"),
                &install_root,
            );
        }

        // 三条 `config_loaded`，各自带着自己那个根。顺序与 `roots` 一致——同一条捕获期里
        // 事件按发生顺序排，而上面那个循环是串行的。
        let events = capture.find("config_loaded");
        assert_eq!(
            events.len(),
            3,
            "三次启动该有三条 `config_loaded`\n{}",
            capture.summary()
        );
        for (i, (event, root)) in events.iter().zip(&roots).enumerate() {
            assert_eq!(
                event.field("install_root"),
                Some(root.root().display().to_string().as_str()),
                "TC{i}：日志报的根与实际锚点不符\n{}",
                capture.summary()
            );
            assert_eq!(
                event.field("path"),
                Some(root.default_config_path().display().to_string().as_str()),
                "TC{i}：日志报的配置路径与实际读的不符\n{}",
                capture.summary()
            );
            assert_eq!(event.field("selected_by"), Some("default location"));
        }

        // 三个根必须互不相同，否则上面那一圈断言可以被一个"永远返回同一个目录"的实现
        // 全部满足。
        let distinct: std::collections::BTreeSet<_> =
            events.iter().map(|e| e.field("install_root")).collect();
        assert_eq!(
            distinct.len(),
            3,
            "三次启动落在了同一个根上\n{}",
            capture.summary()
        );
    }
}
