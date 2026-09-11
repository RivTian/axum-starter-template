//! `{{crate_name}}` 引导入口
//!
//! 与「一个 `#[tokio::main]`」的差别只有一处：runtime 由配置决定形状，所以配置
//! 先于 runtime 存在（加载本身是同步的，前移零代价）；附加 runtime 必须在
//! `block_on` 返回之后同步地关（async 上下文里 drop Runtime 会 panic）。
//! 其余逻辑在 [`boot::boot_strap`]；这里只负责把结果变成进程退出码。

mod boot;
mod cli;
mod rt;
mod signals;
mod state;

use std::process::ExitCode;
use std::time::Duration;

use {{crate_prefix_snake}}_core::SERVICE_NAME;
use {{crate_prefix_snake}}_core::config::ConfigStore;
use {{crate_prefix_snake}}_core::util::BuildInfo;

use cli::CliAction;

/// runtime 自身的收尾宽限：到这一步顶层任务都已被监管器收割，理应已无任务；
/// 短宽限只为兜住面内部 spawn 出去又没随取消收敛的子任务（那是 bug，但要兜住）
const RUNTIME_GRACE: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    // 0. 解析 CLI
    let CliAction::Run { config } = cli::parse(std::env::args().skip(1)) else {
        // release 构建经 {{env_prefix}}_VERSION 注入 tag 版本，未注入回退包版本
        println!("{}", BuildInfo::current().version);
        return ExitCode::SUCCESS;
    };

    // 1. tracing 初始化（全 workspace 仅此一处）
    boot::init_tracing();

    // 第一条日志固定打构建串：现场排查只看日志开头就知道跑的哪个版本。
    // 名字用二进制名而不是包名（`<prefix>-app`）：与自省端点报的是同一个串
    let service = BuildInfo::current().service_description(SERVICE_NAME);
    tracing::info!("{service}");

    // 2. 配置：统一管线 → ConfigStore（watch 写端）；缺文件落盘内嵌模板。
    //    定位要问系统「我这个二进制在哪」，问得到才谈得上加载，故分两步
    let (root, path) = match cli::locate_config(config) {
        Ok(located) => located,
        Err(error) => return fail(&error),
    };
    let config = match ConfigStore::load_or_init(path, root) {
        Ok(config) => config,
        Err(error) => return fail(&error),
    };
    tracing::info!(path = %config.path().display(), "config loaded");

    // 3. runtime：形状来自 [runtime]
    let runtimes = match rt::Runtimes::build(&config.current().runtime) {
        Ok(runtimes) => runtimes,
        Err(error) => return fail(&error),
    };

    // 4. 其余全部在 runtime 里：TLS provider → 存储 → 共享状态 → 任务注册 → monitor
    let executors = runtimes.executors();
    let result = runtimes.block_on(boot::boot_strap(config, executors));

    // 5. runtime 收尾：在 block_on 之外、同步地关（顺序在日志里可见）
    runtimes.shutdown(RUNTIME_GRACE);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error),
    }
}

/// 启动失败的统一出口：tracing 可能尚未初始化（配置加载失败前），stderr 双保险。
fn fail(error: &dyn std::fmt::Display) -> ExitCode {
    tracing::error!(%error, "startup failed");
    eprintln!("{SERVICE_NAME}: {error}");
    ExitCode::FAILURE
}
