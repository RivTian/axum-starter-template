//! 路径锚点：可执行文件所在目录。永不取 cwd。
//!
//! 配置里的相对路径（数据目录、`url_file`、SQLite URL 里的相对路径）一律从同一个锚点派生。
//! CLI 与环境变量只能改"配置文件的位置"，改不了锚点——否则"换个目录启动就读到另一份配置"
//! 这类故障会重新出现。

use std::path::{Path, PathBuf};

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

/// 路径锚点（可执行文件所在目录）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor(PathBuf);

impl Anchor {
    /// 生产入口：当前可执行文件所在目录。
    pub fn from_current_exe() -> Result<Self, Error> {
        let exe = std::env::current_exe().map_err(|err| {
            Error::with_source(ErrorKind::Config, "无法确定当前可执行文件的位置", err)
        })?;
        let dir = exe.parent().ok_or_else(|| {
            Error::config(format!("可执行文件路径没有父目录：`{}`", exe.display()))
        })?;
        Ok(Self(dir.to_path_buf()))
    }

    /// 显式构造（测试与嵌入场景用）。
    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        Self(dir.into())
    }

    /// 锚点目录本身。
    pub fn dir(&self) -> &Path {
        &self.0
    }

    /// 相对路径 → 锚点下的路径；绝对路径原样返回。
    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.0.join(path)
        }
    }

    /// 把 `sqlite:` URL 里的相对路径改写成锚点下的路径；其它形式原样返回。
    ///
    /// 只支持 `sqlite:` 一个 scheme：其余 scheme（以及 `sqlite::memory:`、`sqlite:file:`、
    /// `sqlite://` 这类显式形式）不做猜测，交给存储层去报错或原样使用。
    pub fn rewrite_sqlite_url(&self, url: &str) -> String {
        let Some(rest) = url.strip_prefix("sqlite:") else {
            return url.to_owned();
        };
        if rest.starts_with("//") || rest.starts_with(":memory:") || rest.starts_with("file:") {
            return url.to_owned();
        }
        let (path_part, query) = match rest.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (rest, None),
        };
        if path_part.is_empty() || Path::new(path_part).is_absolute() {
            return url.to_owned();
        }
        let joined = self.join(path_part);
        let joined = joined.to_string_lossy();
        match query {
            Some(query) => format!("sqlite:{joined}?{query}"),
            None => format!("sqlite:{joined}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_keeps_absolute_paths_and_anchors_relative_ones() {
        let anchor = Anchor::from_dir("/opt/service");
        assert_eq!(
            anchor.join("data/app.db"),
            Path::new("/opt/service/data/app.db")
        );
        assert_eq!(anchor.join("/var/lib/app.db"), Path::new("/var/lib/app.db"));
    }

    #[test]
    fn sqlite_relative_paths_are_anchored() {
        let anchor = Anchor::from_dir("/opt/service");
        assert_eq!(
            anchor.rewrite_sqlite_url("sqlite:data/service.db?mode=rwc"),
            "sqlite:/opt/service/data/service.db?mode=rwc"
        );
        assert_eq!(
            anchor.rewrite_sqlite_url("sqlite:data/service.db"),
            "sqlite:/opt/service/data/service.db"
        );
    }

    #[test]
    fn sqlite_explicit_forms_are_left_alone() {
        let anchor = Anchor::from_dir("/opt/service");
        for url in [
            "sqlite::memory:",
            "sqlite:///var/lib/service.db",
            "sqlite:file:service.db?cache=shared",
            "sqlite:/var/lib/service.db",
            "postgres://user@localhost/db",
        ] {
            assert_eq!(anchor.rewrite_sqlite_url(url), url);
        }
    }
}
