//! 配置文件监听：只提供"变了"这个事实，不做去抖、不拥有运行时。
//!
//! - 监听的是配置文件**所在目录**（非递归）：编辑器保存常常是"写临时文件 + rename"，
//!   盯住文件本身会漏事件；
//! - 回调在 notify 自己的线程上执行：装配层负责把它桥进自己的异步世界（并在那里去抖）；
//! - 回调只报"可能变了"，不保证每次都对应一次完整写入——所以装配层的去抖必须是
//!   "先收到事件、等一小段时间、再读文件"，读到的永远是当时的完整内容。

use std::path::Path;

use notify::{RecursiveMode, Watcher};

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

/// 存活的监听器：drop 掉它，监听就停。
#[derive(Debug)]
pub struct FileWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// 监听 `dir` 下的 `file_name`，每次可能的变更调用一次 `on_change`。
pub fn watch_file(
    dir: &Path,
    file_name: &str,
    on_change: impl Fn() + Send + 'static,
) -> Result<FileWatcher, Error> {
    let target = dir.join(file_name);
    let watched_name = file_name.to_owned();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        // 目录事件可能不带路径（罕见），那就保守地当作"可能变了"。
        let hits_target = event.paths.is_empty()
            || event.paths.iter().any(|path| {
                path == &target
                    || path
                        .file_name()
                        .is_some_and(|name| name == watched_name.as_str())
            });
        if hits_target {
            on_change();
        }
    })
    .map_err(|err| {
        Error::with_source(
            ErrorKind::Config,
            format!("无法创建配置文件监听器（目录 `{}`）", dir.display()),
            err,
        )
    })?;

    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(|err| {
            Error::with_source(
                ErrorKind::Config,
                format!("无法监听配置文件目录 `{}`", dir.display()),
                err,
            )
        })?;

    Ok(FileWatcher { _watcher: watcher })
}
