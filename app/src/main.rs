//! 进程入口。
//!
//! # 这个文件为什么这么短
//!
//! 一条表达式，没有分支。所有分支都在 [`run`] 里面，而 `run` 的每一个入参都是**数据**：
//! 进程环境是抄下来的一份快照，信号流是一个可以被换掉的产生器。于是同一条路径在用例里
//! 可以原样跑一遍，只是换掉那两个入参。
//!
//! `main` 里每多一个 `if`，就多一条只有真起进程才能覆盖到的路径——而"靠起进程来验收"
//! 这条路一开始就排除掉了。门禁扫描这个文件里不得出现 `if` / `match` / 循环。
//!
//! # 两处只能在这里取的事实
//!
//! - **`env!("CARGO_BIN_NAME")`**：只在 `[[bin]]` 目标里可见，库里取不到（实测）。
//!   两处对外可见的东西都由它推导——环境变量前缀（`<PREFIX>_CONFIG`）和 `/v1/info` 报的
//!   那个 `service`——于是整个 Rust 源里不需要出现任何项目名字面量。
//!
//!   [`BuildInfo::current`] 因此是**收**一个服务名，不是自己去读一个：这个 crate 的
//!   `CARGO_PKG_NAME` 是 workspace 的内部构造（`<前缀>-app`），把它发到对外契约上等于把
//!   仓库布局公开了；而 `core` 那边连 `env!` 都用不了——它是 lib 目标。
//! - **[`ProcessEnv::capture`]**：`current_exe` / `args_os` / `vars_os` / `is_terminal`
//!   这四件事读的是真实进程。失败**不**在这里分支——`Result` 原样交给 `run`，由它变成一份
//!   报告。
//!
//! # 为什么传的是函数 `os_signal_stream` 而不是一条流
//!
//! 实测：`tokio::signal::unix::signal()` 在 runtime 之外调用会 **panic**——
//! `there is no reactor running, must be called from the context of a Tokio 1.x runtime`。
//! 而 runtime 是 `run` 自己建的，`main` 手上根本没有。
//!
//! 所以 `run` 收的不是一条流，是一个**产生流的函数**，由 `run` 在 runtime 上下文里调用。
//! 写成 `run(env, stops: impl Stream)` 是更自然的形状，但那个签名跑不起来。注入点并
//! 没有变（用例塞 `|| Ok(合成流)`），变的只是注入的是值还是产生值的函数。
//!
//! 注册失败**不兜底**（理由见 [`os_signal_stream`] 的文档）：它是一次启动失败，退出码 1。
//! 这里不写 `unwrap_or_else` 退到一条空流——那等于让一个收不到 SIGTERM 的进程假装自己
//! 健康，而它的结局是被 `SIGKILL`，全部收尾都会被跳过。
//!
//! [`BuildInfo::current`]: service_core::build_info::BuildInfo::current

// `unused_crate_dependencies` 是**逐 target** 判定的：这个 bin 只用到自家的 lib，于是
// `app` 声明的其余十几条依赖在这个 target 看来全是多余的。它们并不多余——它们服务于
// `service_app` 这个 lib target，而 lint 在那边照常生效（`serde` 就是在那边被它抓出来
// 删掉的）。
//
// 这里不写十几行 `use tokio as _;` 去哄它：那些语句不表达任何东西，正是那种「为观感
// 加的构造」。`allow` 至少把"我知道，且原因在这里"写了下来。
#![allow(unused_crate_dependencies)]

use std::process::ExitCode;

use service_app::{ProcessEnv, exit_code, os_signal_stream, run};

fn main() -> ExitCode {
    exit_code(&run(
        ProcessEnv::capture(env!("CARGO_BIN_NAME")),
        os_signal_stream,
    ))
}
