//! 部署路径约定：安装根，以及由它派生的缺省布局。
//!
//! ```text
//! <安装根>/              ← 可执行文件所在目录
//!   config/<服务名>.toml   缺省配置（--config / 环境变量可覆盖位置）
//!   data/<服务名>.db       缺省 SQLite 库（[storage.sqlite].path 可覆盖）
//! ```
//!
//! `config/` 与 `data/` 是**同级**：一个归运维改、一个归进程写，备份与权限策略
//! 也不同。库钻进配置目录里会让「拷一份配置」顺带拷走几百兆数据。
//!
//! # 安装根只有一个来源：可执行文件的位置
//!
//! 不取进程 cwd——cwd 不是进程自己的属性，systemd 拉起的服务 cwd 通常是 `/`，
//! 从别的目录启动同一个二进制又会在那里另造一套目录。
//!
//! 也**不跟着 `--config` 走**：systemd 单元里写 `--config <安装根>/config/x.toml`
//! 只是把缺省值显式写出来，不该因此让库换个地方落。「显式写出缺省路径反而改变
//! 了行为」是最难查的一类故障。代价是把配置挂到别处（容器里挂载一份）时配置里的
//! 相对路径仍相对安装根——那种场景本就该在配置里写绝对路径，且两个路径都会出现
//! 在启动的头几行日志里。

use std::io;
use std::path::{Path, PathBuf};

use crate::SERVICE_NAME;

/// 缺省布局的配置目录名。
pub const CONFIG_DIR: &str = "config";
/// 缺省布局的数据目录名（消费者：`config::StorageConfig::resolve_paths`）。
pub const DATA_DIR: &str = "data";

/// 安装根：可执行文件所在目录。
pub fn install_root() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent().map(PathBuf::from).ok_or_else(|| {
        io::Error::other(format!(
            "可执行文件 {} 没有父目录，无法推断安装根；请用 --config 显式指定配置路径",
            exe.display()
        ))
    })
}

/// 缺省配置文件名 `<服务名>.toml`，与 [`SERVICE_NAME`] 同一个串。
///
/// 不写成字面量：项目名一旦进到字符串里，这一行的宽度就随名字长短在 rustfmt 的
/// 阈值上下翻转，模板便只对部分名字 fmt-clean（门禁见 `scripts/check-fmt-portability.py`）。
pub fn config_file_name() -> String {
    format!("{SERVICE_NAME}.toml")
}

/// 缺省配置文件路径 `<root>/config/<服务名>.toml`。
///
/// 取 `root` 而不是自己调 [`install_root`]：纯函数好测，调用点也看得见基准是谁。
pub fn default_config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIR).join(config_file_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_root_is_the_executable_directory() {
        let root = install_root().expect("测试二进制必然有父目录");
        let exe = std::env::current_exe().unwrap();
        assert!(root.is_absolute(), "{}", root.display());
        assert_eq!(root, exe.parent().unwrap());
    }

    /// 缺省布局钉死：配置在 `config/` 里，文件名跟服务名
    #[test]
    fn default_config_path_sits_in_the_config_dir() {
        let root = Path::new("/opt/svc");
        let path = default_config_path(root);
        let tail = Path::new(CONFIG_DIR).join(config_file_name());
        assert!(path.ends_with(&tail), "{}", path.display());
        assert_eq!(path.parent().and_then(Path::parent), Some(root));
    }

    /// `config/` 与 `data/` 必须同级：库不进配置目录
    #[test]
    fn config_and_data_are_siblings() {
        let root = Path::new("/opt/svc");
        let config_dir = default_config_path(root).parent().unwrap().to_path_buf();
        let data_dir = root.join(DATA_DIR);
        assert_eq!(config_dir.parent(), data_dir.parent(), "同一个安装根下");
        assert_ne!(config_dir, data_dir, "是两个目录，不是一个");
    }
}
