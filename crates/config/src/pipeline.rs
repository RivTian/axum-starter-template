//! 唯一加载管线：定位 → 读文件（缺则落盘内嵌模板）→ 解析 → 环境变量覆盖 → 生效形态。
//!
//! 启动与热重载调用的是同一个 [`load`]；重载只是在此之上做 diff/分类/回滚。只要读文件这件事只有
//! 一个入口，"启动用一套、reload 用另一套"的漂移就不可能发生。

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

use crate::anchor::Anchor;
use crate::embedded;
use crate::env::{self, EnvSource};
use crate::schema::{Config, FileConfig, Notice, NoticeKind};
use crate::source::ConfigSource;

/// 一次加载的结果。
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// 非致命提示（钳位、遮蔽、默认模板落盘失败）：装配层负责打成日志。
    pub notices: Vec<Notice>,
    pub source: ConfigSource,
    /// 命中的是内嵌默认模板（文件原本不存在）。
    pub from_embedded_template: bool,
}

/// 三段入口 → 生效配置。
pub fn load(source: &ConfigSource, anchor: &Anchor, env: &dyn EnvSource) -> Result<Loaded, Error> {
    let mut notices = Vec::new();

    let (text, from_embedded_template) = match std::fs::read_to_string(&source.path) {
        Ok(text) => (text, false),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Err(write_err) = embedded::write_default(&source.path) {
                notices.push(Notice {
                    path: "config".to_owned(),
                    kind: NoticeKind::DefaultNotWritten,
                    detail: format!(
                        "默认配置写不进 `{}`（{write_err}），本次使用内存里的默认值",
                        source.path.display()
                    ),
                });
            }
            (embedded::TEMPLATE.to_owned(), true)
        }
        Err(err) => {
            return Err(Error::with_source(
                ErrorKind::Config,
                format!("读取配置文件 `{}` 失败", source.path.display()),
                err,
            ));
        }
    };

    // 第一次反序列化：直接按文本读，错误里带行列位置（拼错键、类型不对都在这里说清）。
    let file: FileConfig = toml::from_str(&text).map_err(|err| {
        // 解析错误的原文（含行列与出错键名）直接进 message：这是启动期最需要看清的一条信息。
        let detail = err.to_string();
        Error::new(
            ErrorKind::Config,
            format!("配置文件 `{}` 不合法：\n{detail}", source.path.display()),
        )
    })?;

    // 环境变量覆盖：在"树"上做，类型按默认值校验；形状匹配但键不存在 = 错误。
    let defaults = env::defaults_tree();
    let mut tree = toml::Value::try_from(&file).map_err(|err| {
        Error::with_source(ErrorKind::Config, "配置文件无法规范化成 TOML 树", err)
    })?;
    env::apply_overlay(&mut tree, env, &defaults)?;
    let file: FileConfig = tree.try_into().map_err(|err| {
        Error::with_source(
            ErrorKind::Config,
            format!(
                "配置文件 `{}` 在环境变量覆盖后类型不一致",
                source.path.display()
            ),
            err,
        )
    })?;

    // 生效形态：补全 → 校验 → 敏感信息 → 路径锚点 → 钳位。
    let (config, mut config_notices) = Config::from_file(file, anchor, env)?;
    notices.append(&mut config_notices);

    Ok(Loaded {
        config,
        notices,
        source: source.clone(),
        from_embedded_template,
    })
}
