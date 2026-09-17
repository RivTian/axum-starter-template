//! 入口设置：CLI 参数、路径锚点、三段入口与首次配置加载。
//!
//! CLI 只覆盖"配置文件的位置"（`--config`），改不了路径锚点——锚点永远是可执行文件所在目录。

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use {{crate_prefix_snake}}_config::Anchor;
use {{crate_prefix_snake}}_config::Config;
use {{crate_prefix_snake}}_config::ConfigSource;
use {{crate_prefix_snake}}_config::EnvSource;
use {{crate_prefix_snake}}_config::Notice;
use {{crate_prefix_snake}}_config::ProcessEnv;
use {{crate_prefix_snake}}_config::load;
use {{crate_prefix_snake}}_core::Error;

/// 二进制名与简介：放进常量，让 rustfmt 的排版与生成期定下的名字长度无关。
const BIN_NAME: &str = "{{crate_name}}";
const ABOUT: &str = "{{project-name}} 服务";

/// 命令行入口。生成结果不带部署事实，所以这里也没有端口/绑定之类的参数——
/// 那些都在配置文件里（`[http].bind`）。
#[derive(Debug, Parser)]
#[command(name = BIN_NAME, version, about = ABOUT)]
pub struct Args {
    /// 配置文件位置（默认：可执行文件所在目录下的 config.toml）。
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// 解析完成的启动设置。
#[derive(Debug)]
pub struct Settings {
    pub anchor: Anchor,
    pub source: ConfigSource,
    /// 首次加载得到的生效配置。
    pub config: Config,
    /// 管线提示（钳位 / 遮蔽 / 默认模板写不进去）。
    pub notices: Vec<Notice>,
    /// 配置文件原本不存在，用的是内嵌默认模板。
    pub from_embedded_template: bool,
}

/// 生产入口：锚点取当前可执行文件所在目录。
pub fn resolve(args: &Args, env: &Arc<dyn EnvSource>) -> Result<Settings, Error> {
    resolve_with_anchor(args, env, Anchor::from_current_exe()?)
}

/// 显式锚点入口（测试与嵌入场景）：除锚点外与 [`resolve`] 完全同一条路。
pub fn resolve_with_anchor(
    args: &Args,
    env: &Arc<dyn EnvSource>,
    anchor: Anchor,
) -> Result<Settings, Error> {
    let source = ConfigSource::locate(args.config.as_deref(), env.as_ref(), &anchor);
    let loaded = load(&source, &anchor, env.as_ref())?;
    // 注意：这里可能发生在 subscriber 安装之前（首载在 telemetry 之前），所以不在这里打日志；
    // "读的是哪份配置"由 bootstrap 在日志起来之后统一落（见 `configuration loaded`）。
    Ok(Settings {
        anchor,
        source: loaded.source,
        config: loaded.config,
        notices: loaded.notices,
        from_embedded_template: loaded.from_embedded_template,
    })
}

/// 默认的环境变量来源（进程环境）。
pub fn process_env() -> Arc<dyn EnvSource> {
    Arc::new(ProcessEnv)
}
