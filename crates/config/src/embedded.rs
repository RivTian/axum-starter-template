//! 内嵌默认配置模板：缺配置文件时落盘的内容，也是"零配置可起"的依据。
//!
//! 测试会断言它与 [`crate::FileConfig`] 的 `Default` 逐字段一致，所以改 schema 时必须一起改
//! 这个文件（两者不一致会让"删掉某段等价于默认值"这句话失真）。

use std::path::Path;

/// 编译期嵌入的默认配置模板（正文见 `config-template.toml`）。
pub const TEMPLATE: &str = include_str!("../config-template.toml");

/// 把默认模板写到 `path`（父目录不存在则创建）。
pub(crate) fn write_default(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, TEMPLATE)
}
