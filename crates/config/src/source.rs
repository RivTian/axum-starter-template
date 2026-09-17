//! 三段入口：配置文件位置从哪来。
//!
//! 优先级：CLI `--config` > 环境变量 `ENV_PREFIX_CONFIG` > 路径锚点下的 `config.toml`。
//! 前两者只改"文件在哪"，改不了锚点本身——锚点永远是可执行文件所在目录。

use std::path::{Path, PathBuf};

use crate::anchor::Anchor;
use crate::env::{self, EnvSource};

/// 配置文件位置是从哪一段入口拿到的（日志里会带上，便于排查"为什么读的不是我以为的那份"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOrigin {
    /// CLI `--config`。
    Cli,
    /// 环境变量 `ENV_PREFIX_CONFIG`。
    Env,
    /// 路径锚点下的默认位置。
    AnchorDefault,
}

impl SourceOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Env => "env",
            Self::AnchorDefault => "anchor-default",
        }
    }
}

/// 解析出的配置文件位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSource {
    pub path: PathBuf,
    pub origin: SourceOrigin,
}

impl ConfigSource {
    /// 三段入口的优先级在这里一次说清（这是**唯一**决定读哪个文件的地方）。
    ///
    /// CLI 与环境变量的相对路径按调用者所在目录（cwd）解释——那是用户显式给出的路径；
    /// 锚点只约束**默认位置**与配置内部的相对路径。
    pub fn locate(cli: Option<&Path>, env_source: &dyn EnvSource, anchor: &Anchor) -> Self {
        if let Some(path) = cli {
            return Self {
                path: path.to_path_buf(),
                origin: SourceOrigin::Cli,
            };
        }
        if let Some(value) = env_source
            .get(&env::config_env_name())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
        {
            return Self {
                path: PathBuf::from(value),
                origin: SourceOrigin::Env,
            };
        }
        Self {
            path: anchor.join("config.toml"),
            origin: SourceOrigin::AnchorDefault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::MapEnv;

    fn anchor() -> Anchor {
        Anchor::from_dir("/srv/app")
    }

    #[test]
    fn cli_beats_env_beats_anchor_default() {
        let env = MapEnv::new().with(env::config_env_name(), "/etc/from-env.toml");
        let source = ConfigSource::locate(Some(Path::new("/etc/from-cli.toml")), &env, &anchor());
        assert_eq!(source.origin, SourceOrigin::Cli);
        assert_eq!(source.path, PathBuf::from("/etc/from-cli.toml"));

        let source = ConfigSource::locate(None, &env, &anchor());
        assert_eq!(source.origin, SourceOrigin::Env);
        assert_eq!(source.path, PathBuf::from("/etc/from-env.toml"));

        let source = ConfigSource::locate(None, &MapEnv::new(), &anchor());
        assert_eq!(source.origin, SourceOrigin::AnchorDefault);
        assert_eq!(source.path, PathBuf::from("/srv/app/config.toml"));
    }

    #[test]
    fn empty_env_value_falls_through_to_anchor_default() {
        let env = MapEnv::new().with(env::config_env_name(), "   ");
        let source = ConfigSource::locate(None, &env, &anchor());
        assert_eq!(source.origin, SourceOrigin::AnchorDefault);
    }
}
