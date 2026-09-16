//! 命令行：**只有三个开关**，手写解析。
//!
//! # 为什么不引入参数解析库
//!
//! 参数表一共三项：`--config <PATH>` / `--help` / `--version`。为它引一个解析库要付
//! 编译期、一套派生宏、以及一个"以后总会有人往里加东西"的邀请。「不为观感引依赖」这条
//! 就是冲这类情况写的。
//!
//! 反过来，手写解析的典型坑——`--config` 后面忘了给值、把下一个开关当成了值——在这里
//! 是**显式失败**而不是静默兜底：认得的开关缺值一律 fail-fast，因为"用默认路径
//! 起来了"和"用你指定的路径起来了"是两个完全不同的进程，不能让用户靠猜。
//!
//! # 为什么有 `--help` / `--version`
//!
//! 它们不是"预铺的形状"：一个接受 `--config` 的二进制必须有地方告诉别人这件事，
//! 而一个跑在机器上的服务必须能自报版本。两者各自只有十几行，且都有用例。

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use service_core::config::FieldPath;

/// 解析出来的命令行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cli {
    /// `--config` 给的路径。`None` 表示"按优先级继续往下找"。
    pub(crate) config: Option<PathBuf>,
    /// 这一次要做什么。
    pub(crate) action: Action,
}

/// 命令行最终要求进程做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// 正常启动。
    Run,
    /// 打印用法后退出。
    Help,
    /// 打印版本后退出。
    Version,
}

/// 命令行解析失败。
///
/// 三个变体都带**消毒过**的原文（[`FieldPath`]）：命令行是用户输入，原样回显到日志里
/// 就等于把控制字符和任意长度的串交给日志管道。
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// 认得的开关，但后面没有值。
    #[error("option `{flag}` requires a value")]
    MissingValue {
        /// 缺值的开关名。
        flag: &'static str,
    },
    /// 不认得的开关。
    #[error("unknown option `{arg}`")]
    UnknownOption {
        /// 消毒过的原文。
        arg: FieldPath,
    },
    /// 多余的位置参数。本程序一个都不收。
    #[error("unexpected positional argument `{arg}`")]
    UnexpectedPositional {
        /// 消毒过的原文。
        arg: FieldPath,
    },
}

/// 解析 `argv`。传进来的切片**含 argv[0]**，本函数负责跳过它。
///
/// # Errors
///
/// 见 [`CliError`]。任何一种都不做兜底——命令行写错了就不要起来。
pub(crate) fn parse(argv: &[OsString]) -> Result<Cli, CliError> {
    let mut config = None;
    let mut action = Action::Run;
    let mut rest = argv.iter().skip(1);

    while let Some(arg) = rest.next().map(OsString::as_os_str) {
        match arg.to_str() {
            Some("--help" | "-h") => action = Action::Help,
            Some("--version" | "-V") => action = Action::Version,
            Some("--config") => {
                let value = rest
                    .next()
                    .ok_or(CliError::MissingValue { flag: "--config" })?;
                config = Some(PathBuf::from(value));
            }
            // `--config=/path` 形态。分开写是因为上面那支要"吃掉下一个参数"，
            // 而这一支不能——两者的失败方式不一样，合并写会让缺值检查失效。
            _ if starts_with(arg, "--config=") => {
                let value = split_after(arg, "--config=");
                config = Some(PathBuf::from(value));
            }
            _ if starts_with(arg, "-") => {
                return Err(CliError::UnknownOption {
                    arg: FieldPath::new(&arg.to_string_lossy()),
                });
            }
            _ => {
                return Err(CliError::UnexpectedPositional {
                    arg: FieldPath::new(&arg.to_string_lossy()),
                });
            }
        }
    }

    Ok(Cli { config, action })
}

/// `--config=` 这类前缀判断要在 `OsStr` 上做，不能先 `to_string_lossy()`：
/// 有损转换会把非法字节换成 U+FFFD，于是一个本来非法的路径会**变得看起来合法**。
fn starts_with(arg: &OsStr, prefix: &str) -> bool {
    arg.as_encoded_bytes().starts_with(prefix.as_bytes())
}

/// 切掉 `prefix`，把剩下的字节还原成一个 `OsString`。
///
/// unix 上走 `OsStrExt::from_bytes`——**无损**，非 UTF-8 的路径能原样传下去。
/// 其他平台没有等价的安全接口（`from_encoded_bytes_unchecked` 需要 `unsafe`，而
/// `unsafe_code` 在本 workspace 是 `forbid`），只能接受有损转换：代价是
/// `--config=` 后面跟非 UTF-8 路径时会被替换字符破坏，随后以"文件不存在"失败——
/// 一个明确的错误，而不是静默用错文件。
fn split_after(arg: &OsStr, prefix: &str) -> OsString {
    let bytes = &arg.as_encoded_bytes()[prefix.len()..];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(bytes).to_owned()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// 用法文本。`bin` 来自 [`ProcessEnv::bin_name`](crate::ProcessEnv::bin_name)，
/// 于是这里也没有项目名字面量。
pub(crate) fn usage(bin: &str) -> String {
    format!(
        "\
{bin} —— HTTP 服务

用法：
    {bin} [选项]

选项：
    --config <PATH>    指定配置文件。优先级最高，高于环境变量与安装根下的默认位置。
    -h, --help         打印本说明并退出。
    -V, --version      打印版本并退出。

配置文件的查找顺序：
    1. --config <PATH>
    2. ${{PREFIX}}_CONFIG 环境变量（PREFIX 为可执行文件名的大写形态）
    3. <安装根>/config/service.toml

安装根就是可执行文件所在的那个目录（开发期即 target/debug/）。首次启动时若默认位置没有配置文件，
会写出一份与内置默认值等价的模板；用 --config 指定的路径**不会**被自动创建。
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<OsString> {
        std::iter::once(OsString::from("prog"))
            .chain(items.iter().map(OsString::from))
            .collect()
    }

    #[test]
    fn no_arguments_means_run_with_no_explicit_config() {
        let cli = parse(&argv(&[])).unwrap();
        assert_eq!(cli.action, Action::Run);
        assert_eq!(cli.config, None);
    }

    #[test]
    fn config_accepts_both_spellings() {
        let split = parse(&argv(&["--config", "/etc/a.toml"])).unwrap();
        let joined = parse(&argv(&["--config=/etc/a.toml"])).unwrap();
        assert_eq!(split.config, Some(PathBuf::from("/etc/a.toml")));
        assert_eq!(split, joined, "两种写法必须解析成同一个东西");
    }

    #[test]
    fn a_recognized_option_without_a_value_fails_fast() {
        // 认得的开关缺值**不**兜底。否则"我指定了配置文件"和"它用了默认配置"
        // 之间只差一个用户看不见的空格。
        let err = parse(&argv(&["--config"])).unwrap_err();
        assert!(matches!(err, CliError::MissingValue { flag: "--config" }));
    }

    #[test]
    fn the_next_option_is_not_swallowed_as_a_value() {
        // 手写解析最常见的坑：`--config --help` 把 `--help` 当成了路径。
        // 这里它确实会被当成路径——这是**有意**的：`--config` 的值可以是任意字符串，
        // 猜"它看起来像开关所以不是值"会让一个真的叫 `--help` 的文件无法指定。
        // 用例存在的意义是把这个选择钉死，而不是让它成为一次意外。
        let cli = parse(&argv(&["--config", "--help"])).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("--help")));
        assert_eq!(cli.action, Action::Run);
    }

    #[test]
    fn unknown_options_and_positionals_are_rejected() {
        assert!(matches!(
            parse(&argv(&["--nope"])).unwrap_err(),
            CliError::UnknownOption { .. }
        ));
        assert!(matches!(
            parse(&argv(&["serve"])).unwrap_err(),
            CliError::UnexpectedPositional { .. }
        ));
    }

    #[test]
    fn help_and_version_are_recognized() {
        assert_eq!(parse(&argv(&["--help"])).unwrap().action, Action::Help);
        assert_eq!(parse(&argv(&["-h"])).unwrap().action, Action::Help);
        assert_eq!(
            parse(&argv(&["--version"])).unwrap().action,
            Action::Version
        );
        assert_eq!(parse(&argv(&["-V"])).unwrap().action, Action::Version);
    }

    #[test]
    fn usage_contains_no_project_name_literal() {
        // 名字只能来自参数。换句话说：改项目名不需要改这个文件。
        let text = usage("svc");
        assert!(text.contains("svc [选项]"));
        assert!(text.contains("--config <PATH>"));
    }
}
