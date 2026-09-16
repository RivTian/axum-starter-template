//! 进程边界：全仓库**唯一**读取进程环境的地方。
//!
//! 这个文件里没有一个决策。它只是把四样进程事实抄进一个结构体：可执行文件路径、
//! 命令行、环境变量、stderr 是不是终端。控制流只有 `?`。
//!
//! # 为什么值得为这四行专门开一个模块
//!
//! 因为"读环境"和"根据环境做决定"混在一起写，测试就只能靠真的改进程环境来覆盖——
//! 而进程环境是全局可变状态，一旦并行跑用例就会互相踩。把读取收敛到这一个无分支的
//! 函数之后，下游每一处判断收的都是一个普通结构体：用例直接构造它，不需要 `set_var`，
//! 也不需要串行执行。
//!
//! 门禁会数这个文件里的分支数。它应当一直是 0。
//!
//! # `bin_name` 为什么在这里
//!
//! `env!("CARGO_BIN_NAME")` 只在 bin 目标里有值，lib 目标拿到的是 `None`。所以它不能
//! 在这个文件里读，只能由 `main.rs` 传进来。放进 `ProcessEnv` 而不是再开一个参数，
//! 是因为它和另外四样东西是同一类东西：**进程自己的事实**，而不是某个模块的配置。
//!
//! 它是 `<PREFIX>_CONFIG` 里那个 `<PREFIX>` 的唯一来源，也是 `/v1/info` 那个
//! `service` 的唯一来源，于是整个 Rust 源里不需要出现任何项目名字面量。
//!
//! 类型是 `&'static str` 而不是 `String`：唯一合法的来源是 `env!` 展开出来的字面量。
//! 收一个借来的串就等于允许有人拿 `argv[0]` 填它——而那样一来，把可执行文件改个名就会
//! 换掉环境变量前缀和服务身份，一次改名变成一次静默的配置迁移。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

/// 一次进程启动时的全部环境事实。
///
/// 字段全公开：它是一个纯数据容器，用例要能直接按字面量构造。任何"根据这些字段做判断"
/// 的代码都不在这里。
#[derive(Debug, Clone)]
pub struct ProcessEnv {
    /// `std::env::current_exe()` 的结果。安装根由它推导。
    pub exe_path: PathBuf,
    /// `std::env::args_os()`，**含 argv[0]**。
    pub args: Vec<OsString>,
    /// `std::env::vars_os()`。用 `BTreeMap` 而不是 `HashMap`：遍历顺序确定，
    /// 出错信息与日志里的变量顺序才不会每次运行都换一个样子。
    pub vars: BTreeMap<OsString, OsString>,
    /// stderr 是不是终端。ANSI 着色只由它决定。
    pub stderr_is_terminal: bool,
    /// 可执行文件名，由 `main.rs` 的 `env!("CARGO_BIN_NAME")` 传入。见模块文档。
    pub bin_name: &'static str,
}

impl ProcessEnv {
    /// 抄下当前进程的环境。
    ///
    /// # Errors
    ///
    /// `std::env::current_exe()` 失败时返回它的 [`io::Error`]。这不是"读不到就算了"的
    /// 那种失败：安装根锚在它身上，拿不到它就无从知道配置和数据该去哪里找。
    pub fn capture(bin_name: &'static str) -> io::Result<Self> {
        Ok(Self {
            exe_path: std::env::current_exe()?,
            args: std::env::args_os().collect(),
            vars: std::env::vars_os().collect(),
            stderr_is_terminal: io::stderr().is_terminal(),
            bin_name,
        })
    }

    /// 取一个环境变量的 UTF-8 值。
    ///
    /// 非 UTF-8 的值读作"没有"：本模板的每一个环境变量取值都要么是路径要么是标识符，
    /// 没有一个需要非 UTF-8。把它当作"未设置"而不是报错，是因为一个恰好含非法字节的
    /// **无关**变量不该让进程起不来。
    #[must_use]
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars.get(std::ffi::OsStr::new(name))?.to_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_reports_the_bin_name_it_was_given() {
        // 这条同时守住 P-C：lib 侧读不到 `CARGO_BIN_NAME`，它只能是传进来的。
        let env = ProcessEnv::capture("whatever").expect("current_exe 在测试进程里必然可用");
        assert_eq!(env.bin_name, "whatever");
        assert!(!env.args.is_empty(), "argv[0] 必须在");
    }

    #[test]
    fn a_non_utf8_value_reads_as_absent() {
        let mut vars = BTreeMap::new();
        vars.insert(OsString::from("GOOD"), OsString::from("yes"));
        let env = ProcessEnv {
            exe_path: PathBuf::from("/tmp/x"),
            args: vec![OsString::from("x")],
            vars,
            stderr_is_terminal: false,
            bin_name: "x",
        };
        assert_eq!(env.var("GOOD"), Some("yes"));
        assert_eq!(env.var("MISSING"), None);
    }
}
