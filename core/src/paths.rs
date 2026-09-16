//! 安装根：全进程**唯一**的路径锚点。
//!
//! 锚点 = 可执行文件所在目录。不是 cwd，也不是配置文件所在目录。
//!
//! 为什么两个都不要：
//!
//! - **cwd**：它由启动者决定，同一份安装在 `systemd` 下、在 shell 里、在 `cargo run` 下
//!   会取到三个不同的值。用它当锚点，"换个目录启动读到另一份配置"就成了一个必须靠运维
//!   记住的规矩。这里把它**结构性排除**：`std::env::current_dir` 在整个 workspace 里一次
//!   都不出现，由门禁逐字节证明——这比"试了三个 cwd 结果一样"强，后者只证明了被试的那三个。
//! - **配置文件所在目录**：`--config /tmp/x.toml` 会让 `data/` 跑到 `/tmp/data/`。一种常见的写法
//!   就是这么做的，于是"指定一份配置来复现问题"顺带把数据目录也搬走了。这里 `--config`
//!   **只改读哪个文件**，不改锚点。
//!
//! 这个模块全是纯函数：输入是一个 `Path`，不读环境、不碰文件系统。真实的
//! `current_exe()` 调用在 `app` 的 `ProcessEnv::capture()` 里，那一层无分支无决策。
//! 判断全在这里，因此可测。

use std::path::{Path, PathBuf};

/// 配置目录名。运维改这一份。
const CONFIG_DIR: &str = "config";
/// 数据目录名。进程写这一份。
const DATA_DIR: &str = "data";
/// 缺省配置文件名。
const CONFIG_FILE: &str = "service.toml";

/// 从可执行文件路径推出安装根。
///
/// 取父目录。没有父目录（`exe` 是根，或是个裸文件名）时退回 `.`——**不 panic**：
/// 这条路径来自 `current_exe()`，是进程事实而非用户输入，在这里 panic 等于把一个
/// 罕见的部署形态变成起不来。
#[must_use]
pub fn install_root_from(exe: &Path) -> PathBuf {
    exe.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// `<install_root>/config`。
#[must_use]
pub fn config_dir(install_root: &Path) -> PathBuf {
    install_root.join(CONFIG_DIR)
}

/// `<install_root>/data`。
///
/// 与 `config/` **同级**而不是它的子目录：运维改 / 进程写，权限与备份策略不同。
#[must_use]
pub fn data_dir(install_root: &Path) -> PathBuf {
    install_root.join(DATA_DIR)
}

/// `<install_root>/config/service.toml`——三段优先级里最后一档的缺省路径。
#[must_use]
pub fn default_config_path(install_root: &Path) -> PathBuf {
    config_dir(install_root).join(CONFIG_FILE)
}

/// 把配置里的相对路径锚到安装根。
///
/// 绝对路径原样返回：运维显式写了绝对路径就是要脱离安装根，那是他的意图，不该被改写。
#[must_use]
pub fn resolve_against_root(install_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        install_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_root_is_the_executable_directory() {
        let cases: &[(&str, &str)] = &[
            ("/opt/svc/bin/my-service", "/opt/svc/bin"),
            ("/usr/local/bin/x", "/usr/local/bin"),
            ("/target/debug/my-service", "/target/debug"),
        ];
        for (i, (exe, expected)) in cases.iter().enumerate() {
            assert_eq!(
                install_root_from(Path::new(exe)),
                PathBuf::from(expected),
                "TC{i} ({exe}) 的安装根不符"
            );
        }
    }

    #[test]
    fn bare_file_name_falls_back_to_dot_instead_of_panicking() {
        // 这条路径来自 current_exe()，是进程事实。在这里 panic 等于把罕见部署形态变成起不来。
        assert_eq!(
            install_root_from(Path::new("my-service")),
            PathBuf::from(".")
        );
    }

    #[test]
    fn config_and_data_are_siblings() {
        let root = Path::new("/opt/svc");
        let cfg = config_dir(root);
        let data = data_dir(root);
        assert_eq!(cfg.parent(), data.parent(), "config 与 data 必须同级");
        assert_ne!(cfg, data);
        assert!(
            !data.starts_with(&cfg),
            "data 不能落在 config 之下——两者的权限与备份策略不同"
        );
        assert_eq!(
            default_config_path(root),
            PathBuf::from("/opt/svc/config/service.toml")
        );
    }

    #[test]
    fn relative_paths_anchor_to_root_and_absolute_ones_do_not() {
        let root = Path::new("/opt/svc");
        let cases: &[(&str, &str)] = &[
            ("data/service.sqlite3", "/opt/svc/data/service.sqlite3"),
            ("./x.db", "/opt/svc/./x.db"),
            ("/var/lib/svc/x.db", "/var/lib/svc/x.db"),
        ];
        for (i, (input, expected)) in cases.iter().enumerate() {
            assert_eq!(
                resolve_against_root(root, Path::new(input)),
                PathBuf::from(expected),
                "TC{i} ({input}) 的锚定结果不符"
            );
        }
    }

    #[test]
    fn a_different_anchor_moves_everything_together() {
        // 编排层会用三份不同的 exe_path 跑同样的断言；这里先在纯函数层面钉住。
        let a = install_root_from(Path::new("/opt/a/bin/svc"));
        let b = install_root_from(Path::new("/opt/b/bin/svc"));
        assert_ne!(default_config_path(&a), default_config_path(&b));
        assert_ne!(data_dir(&a), data_dir(&b));
        assert!(default_config_path(&a).starts_with(&a));
        assert!(data_dir(&a).starts_with(&a));
    }
}
