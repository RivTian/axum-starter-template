//! 热重载事务：同一条管线出候选 → 叶子级 diff → 三档分类 → cold 回滚 → 一次发布。
//!
//! 三条纪律：
//! - 失败保留 last-good：候选构造的任何一步失败都返回错误，`running` 不变（不半套用）；
//! - cold 回滚：未登记档位的路径按 cold 处理并记入 `unclassified`（安全方向）；
//! - 报告列出路径（不是值）：装配层把报告打成一条日志，路径清单足以定位"什么被改回去了"。

use std::sync::Arc;

use {{crate_prefix_snake}}_core::{Error, ErrorKind};

use crate::anchor::Anchor;
use crate::env::EnvSource;
use crate::pipeline;
use crate::schema::{Config, Notice, NoticeKind};
use crate::source::ConfigSource;
use crate::tier::{self, Tier};

/// 重载报告：都是叶子路径，不含取值。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    /// 可热：已经生效。
    pub applied: Vec<String>,
    /// 半热：已写入共享句柄，下次使用时生效。
    pub next_use: Vec<String>,
    /// 不可热：已回滚为运行值，需要重启进程才能生效。
    pub restart_required: Vec<String>,
    /// 未在 `tier` 里登记档位、按 cold 兜底处理的路径（正常应为空；非空是模板 bug）。
    pub unclassified: Vec<String>,
}

impl ReloadReport {
    /// 有变化发生。
    pub fn changed(&self) -> bool {
        !(self.applied.is_empty() && self.next_use.is_empty() && self.restart_required.is_empty())
    }
}

/// 一次重载的结果：新配置 + 报告 + 提示。
#[derive(Debug)]
pub struct ReloadOutcome {
    pub config: Config,
    pub report: ReloadReport,
    pub notices: Vec<Notice>,
}

/// 重载器：持有运行值，保证"发布的永远是 last-good 或完整候选"。
pub struct Reloader {
    source: ConfigSource,
    anchor: Anchor,
    env: Arc<dyn EnvSource>,
    running: Config,
}

impl Reloader {
    pub fn new(
        source: ConfigSource,
        anchor: Anchor,
        env: Arc<dyn EnvSource>,
        running: Config,
    ) -> Self {
        Self {
            source,
            anchor,
            env,
            running,
        }
    }

    /// 当前生效的配置（重载失败时它不变）。
    pub fn running(&self) -> &Config {
        &self.running
    }

    /// 跑一次重载。Err 表示候选不合法：什么都不改。
    pub fn reload(&mut self) -> Result<ReloadOutcome, Error> {
        let loaded = pipeline::load(&self.source, &self.anchor, self.env.as_ref())?;
        let mut candidate_tree = toml::Value::try_from(&loaded.config).map_err(|err| {
            Error::with_source(ErrorKind::Config, "候选配置无法规范化成 TOML 树", err)
        })?;
        let running_tree = toml::Value::try_from(&self.running).map_err(|err| {
            Error::with_source(ErrorKind::Config, "运行配置无法规范化成 TOML 树", err)
        })?;

        let mut report = ReloadReport::default();
        for path in tier::different_leaves(&running_tree, &candidate_tree) {
            match tier::classify(&path) {
                Tier::Hot => report.applied.push(path),
                Tier::Semi => report.next_use.push(path),
                Tier::Cold => {
                    tier::copy_leaf(&running_tree, &mut candidate_tree, &path);
                    report.restart_required.push(path);
                }
                Tier::Unknown => {
                    tier::copy_leaf(&running_tree, &mut candidate_tree, &path);
                    report.restart_required.push(path.clone());
                    report.unclassified.push(path);
                }
            }
        }

        let mut notices = loaded.notices;
        if !report.unclassified.is_empty() {
            notices.push(Notice {
                path: "config".to_owned(),
                kind: NoticeKind::UnclassifiedPath,
                detail: format!(
                    "这些配置叶子没有登记热度档位，已按不可热处理（回滚 + 待重启）：{}",
                    report.unclassified.join("、")
                ),
            });
        }

        let next: Config = candidate_tree.try_into().map_err(|err| {
            Error::with_source(ErrorKind::Config, "回滚 cold 段之后的配置类型不一致", err)
        })?;
        self.running = next.clone();
        Ok(ReloadOutcome {
            config: next,
            report,
            notices,
        })
    }
}
