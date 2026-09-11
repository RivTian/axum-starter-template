//! CLI 解析
//!
//! 两个参数不值一个 clap：`--version` 与 `--config <path>`。其余参数一律忽略
//! （部署脚本带着的历史参数不该让进程起不来）。
//!
//! 配置文件路径的取值顺序：`--config` > `${{env_prefix}}_CONFIG` >
//! 缺省布局 `<安装根>/config/<服务名>.toml`（布局与安装根的定义见 core 的 `util::paths`）。

use std::path::PathBuf;

use {{crate_prefix_snake}}_core::SERVICE_NAME;
use {{crate_prefix_snake}}_core::util::{default_config_path, install_root};

/// CLI 动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    Run { config: Option<PathBuf> },
    PrintVersion,
}

/// 解析参数（不含 argv[0]）。只看第一个参数是否为版本请求。
pub fn parse(args: impl IntoIterator<Item = String>) -> CliAction {
    let args: Vec<String> = args.into_iter().collect();

    if matches!(
        args.first().map(String::as_str),
        Some("--version" | "-v" | "-V")
    ) {
        return CliAction::PrintVersion;
    }

    let mut config = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                config = it.next().map(PathBuf::from);
                if config.is_none() {
                    eprintln!("{SERVICE_NAME}: {arg} 需要一个路径参数，已忽略");
                }
            }
            other => {
                if let Some(path) = other.strip_prefix("--config=") {
                    config = Some(PathBuf::from(path));
                }
                // 其余参数静默忽略
            }
        }
    }
    CliAction::Run { config }
}

/// 环境变量名：与 `BuildInfo` 的注入变量同一前缀，同一台机器上多个服务互不串
const CONFIG_ENV: &str = "{{env_prefix}}_CONFIG";

/// 安装根 + 配置文件路径。
///
/// 两者一起定出来：都要先问「我这个二进制在哪」，失败也该在同一处报。
/// `--config` 与环境变量**只改配置文件位置，不动安装根**——单元文件里把缺省路径
/// 显式写出来是常见做法，不该因此让 `data/` 换个地方落。它们里的相对路径相对进程
/// cwd：那是人在 shell 里敲的，就该是 shell 的语义。
pub fn locate_config(cli: Option<PathBuf>) -> std::io::Result<(PathBuf, PathBuf)> {
    let root = install_root()?;
    let path = cli
        .or_else(|| std::env::var_os(CONFIG_ENV).map(PathBuf::from))
        .unwrap_or_else(|| default_config_path(&root));
    Ok((root, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn version_flags_win_in_first_position() {
        for flag in ["--version", "-v", "-V"] {
            assert_eq!(parse(args(&[flag])), CliAction::PrintVersion);
        }
        // 不在第一位就不是版本请求
        assert!(matches!(
            parse(args(&["--config", "x", "--version"])),
            CliAction::Run { .. }
        ));
    }

    #[test]
    fn config_flag_in_both_forms() {
        assert_eq!(
            parse(args(&["--config", "/etc/x.toml"])),
            CliAction::Run {
                config: Some(PathBuf::from("/etc/x.toml"))
            }
        );
        assert_eq!(
            parse(args(&["--config=/etc/y.toml"])),
            CliAction::Run {
                config: Some(PathBuf::from("/etc/y.toml"))
            }
        );
    }

    #[test]
    fn unknown_args_are_ignored() {
        assert_eq!(
            parse(args(&["--legacy", "flag"])),
            CliAction::Run { config: None }
        );
    }

    /// `--config` 覆盖配置文件位置，但**不**改安装根
    #[test]
    fn cli_path_overrides_the_file_but_not_the_root() {
        let (root, path) = locate_config(Some(PathBuf::from("a.toml"))).unwrap();
        assert_eq!(path, PathBuf::from("a.toml"));
        assert_eq!(root, install_root().unwrap(), "安装根不跟着 --config 走");
    }

    #[test]
    fn default_config_sits_under_the_install_root() {
        // 不在测试里设环境变量：进程级状态会串到并行用例
        if std::env::var_os(CONFIG_ENV).is_some() {
            return;
        }
        let (root, path) = locate_config(None).expect("缺省路径应可推导");
        // 钉住的是「由安装根派生、绝对、不依赖 cwd」，不是具体目录：测试二进制与
        // app 二进制不在同一个目录下，写死目录只会把这条用例钉在 target/debug/deps 上
        assert_eq!(path, default_config_path(&root));
        assert!(path.is_absolute(), "{}", path.display());
    }
}
