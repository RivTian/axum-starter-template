//! 配置里路径字段的公共解析规则。
//!
//! 单独成模块只为一件事：所有段的路径字段规则必须逐字相同。各段自己写一份迟早
//! 漂移，而这条规则漂移的表现是**部署环境下静默读错目录**，本地怎么试都复现不出来。

use std::path::{Path, PathBuf};

/// 相对路径一律相对**安装根**（`util::install_root`）解析，不是进程 cwd，
/// 也不是配置文件所在目录——后者会把 `data/` 拽进 `config/` 里。
///
/// 两处原样返回：
/// - 空串——它是「未指定」而非「未解析」，由各段自己决定缺省怎么推导；
/// - 绝对路径——已经无歧义，再拼一次只会拼坏。
pub(super) fn resolve_relative(value: &mut String, root: &Path) {
    if value.is_empty() {
        return;
    }
    let path = PathBuf::from(&*value);
    if path.is_absolute() {
        return;
    }
    *value = root.join(path).to_string_lossy().into_owned();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_is_joined_onto_the_install_root() {
        let mut value = String::from("data/app.db");
        resolve_relative(&mut value, Path::new("/etc/svc"));
        assert_eq!(value, "/etc/svc/data/app.db");
    }

    #[test]
    fn absolute_and_empty_are_left_alone() {
        let mut absolute = String::from("/var/lib/app.db");
        resolve_relative(&mut absolute, Path::new("/etc/svc"));
        assert_eq!(absolute, "/var/lib/app.db");

        let mut empty = String::new();
        resolve_relative(&mut empty, Path::new("/etc/svc"));
        assert_eq!(empty, "");
    }
}
