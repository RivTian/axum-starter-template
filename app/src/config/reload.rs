//! 配置重载：SIGHUP 之后发生的事。
//!
//! 一次重载是一个**事务**：要么整批换上去，要么一个字段都不动。中间态不存在，因为
//! 热重载的全部意义就是"配置文件里写的"与"进程里生效的"恒等——半套用会让这两者分叉，
//! 而分叉之后运维手上就再没有任何一处可信的真值了。
//!
//! # 触发只有 SIGHUP，没有 HTTP 端点
//!
//! 「换配置」是一次运维动作，不是一次业务请求。做成端点就得回答鉴权问题，而这个模板
//! 的系统端点是不鉴权的；SIGHUP 的权限边界由操作系统给：能给进程发信号的人，
//! 本来就能改它的配置文件。所以这个模块里没有任何一行与 `api` 相关的东西。
//!
//! # 重载不重新回答「读哪个文件」
//!
//! [`ReloadSource`] 里的路径与环境变量快照都是**启动时定下来的**，运行期不再重算。
//!
//! 路径：三段优先级（`--config` / `<PREFIX>_CONFIG` / 安装根缺省位置）取值全部来自进程
//! 启动的那一刻。重算要么得到同一个答案，要么意味着有人在运行期改了进程环境——而
//! `std::env::set_var` 在 Rust 2024 里是 `unsafe`，本模板一次都没用。于是重算是纯粹的
//! 多余，代价却是得让整个 [`ProcessEnv`] 活到进程结束。
//!
//! 环境变量：`${VAR}` 的展开用的是同一份快照，理由同上。这条有一个必须写进 README 的
//! 后果：**改环境变量要重启，SIGHUP 不管用**。
//!
//! [`ProcessEnv`]: crate::boot::ProcessEnv
//!
//! # single-flight 是 `&mut` 给的，不是锁给的
//!
//! 「同时只有一次重载在跑」是硬要求。这里不用 `Mutex`、不用 `AtomicBool`，因为
//! [`ReloadSource::reload`] 要一个 `&mut ConfigPublisher`，而 [`ConfigPublisher`] 不是
//! `Clone`、整个进程里只有一份、被编排循环独占。借用检查器已经把"两次重载重叠"排除了，
//! 再加一把锁只是在同一件事上写第二遍规则——两遍规则迟早会不一致。
//!
//! 随之而来的是：这个函数是**同步**的，它会占住编排循环直到读完解析完。约束它的不是
//! 异步，是[大小上限](super::bootstrap::MAX_CONFIG_BYTES)；完整理由写在
//! [`super::bootstrap`] 的模块文档里。
//!
//! # 这个文件里没有 `std::fs`
//!
//! 唯一一次文件访问走的是 [`bootstrap::read_limited`]，它只读。一个经典的 bug 是让重载复用
//! 启动的入口，于是「把配置文件删掉再 SIGHUP」会静默地把内嵌模板重建出来、然后报告
//! "重载成功"——运维以为自己的改动生效了，实际上配置被换回了出厂值。
//!
//! 堵死它的不是一句注释：`materialize_default` 是 [`super::bootstrap`] 的私有函数，只有
//! [`bootstrap::load`] 调得到；本模块调的是 [`bootstrap::parse`]，而 `parse` 不碰文件
//! 系统。"文件缺失 = 失败"因此是类型与可见性的结论，不是一个记得写的分支。
//! `reload_never_recreates_a_deleted_config_file` 是它的行为证据。
//!
//! [`bootstrap::read_limited`]: super::bootstrap::read_limited
//! [`bootstrap::load`]: super::bootstrap::load
//! [`bootstrap::parse`]: super::bootstrap::parse
//!
//! # 重载失败不是停机理由
//!
//! 所以 [`ReloadSource::reload`] 返回 [`ReloadOutcome`] 而**不是** `Result`：一个
//! `Result` 摆在编排循环里，迟早有人顺手写个 `?`，一份手抖存错的配置就变成了一次计划外
//! 的停机。四个分支在类型上一视同仁，调用方只能挨个处理。
//!
//! # 日志在这里记，不像 [`super::bootstrap`] 那样交出去
//!
//! `bootstrap` 跑在 subscriber 装起来之前，只能把事实作为数据返回。重载发生在服务已经
//! 跑起来之后，subscriber 早就在了。四条记录（成功 / 无变化 / 拒绝 / 失败）是这个模块
//! 的对外契约的一部分，写在一处才不会有人漏记一种。返回值仍然带着同样的事实，于是用例
//! 断言的是值，不是日志。

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use service_core::config::{
    ClampRecord, ConfigPublisher, Generation, ReloadDecision, evaluate_reload,
};
use tracing::{error, info, warn};

use super::bootstrap::{self, BootstrapError, PathSource};
use crate::boot::ProcessEnv;

/// 一次重载读什么、按什么展开。**启动时定下来，运行期只读**。
pub(crate) struct ReloadSource {
    /// 启动时选中的那个文件。重载不重新选路径。
    path: PathBuf,
    /// 它当初是怎么被选中的。只用于把出错信息说完整：
    /// 「cannot read config file `X` (selected via environment)」比只报路径好查得多。
    source: PathSource,
    /// 启动时抄下的环境变量（UTF-8 视图）。
    vars: BTreeMap<String, String>,
    /// 相对路径锚在哪儿。
    install_root: PathBuf,
}

/// 手写 `Debug`：[`Self::vars`] 是整个进程的环境变量快照，里面可能有密钥。
///
/// 一个结构体被 `#[derive(Debug)]` 之后，它会顺着任何一次 `?self` 或 panic 消息漏出去，
/// 而那两处都不在本模块的视线里。这里只报变量**条数**——排查"展开为什么没生效"需要的是
/// "快照是不是空的"，不是每一条的值。
impl fmt::Debug for ReloadSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReloadSource")
            .field("path", &self.path)
            .field("source", &self.source)
            .field("vars", &format_args!("<{} entries>", self.vars.len()))
            .field("install_root", &self.install_root)
            .finish()
    }
}

/// 一次重载的结论。
///
/// 四个分支覆盖了 SIGHUP 之后可能发生的全部事情，且**没有一个是停机理由**。
///
/// # 为什么带着载荷，哪怕编排层一个字段都不读
///
/// 编排层只按**分支**处理（`lifecycle::r#loop` 里那个无 `_` 臂的 `match`），载荷是给用例
/// 的——理由见下面 `clamps` 那条：兜底不能静默，要的是一个能 `assert_eq!` 的值。于是非 test 构建里
/// 这些字段确实"没人读"，`dead_code` 会逐条报出来。
///
/// 这里选 `allow` 而不是把载荷删掉：删掉之后，"钳位了哪几个字段""待重启清单是什么"就只
/// 剩日志一条路，而抓日志断言在本项目里是判过不合格的证据形态。也不选给每个字段挂一个
/// 假读取——那是「为观感加的构造」。
#[cfg_attr(not(test), allow(dead_code, reason = "载荷是用例的断言口，见类型文档"))]
#[derive(Debug)]
pub(crate) enum ReloadOutcome {
    /// 换上去了。
    Applied {
        /// 换上之后的生成号。
        generation: Generation,
        /// 立刻生效的字段。
        hot: Vec<&'static str>,
        /// 要等对应的面重启才生效的字段。
        semi: Vec<&'static str>,
        /// 这一批里被钳位的值。
        ///
        /// 带出来而不是只记一条日志，是因为「不许静默」要的是一个能
        /// `assert_eq!` 的值；一条 `warn!` 只能靠抓日志来断言。
        clamps: Vec<ClampRecord>,
    },
    /// 解析成功，但与生效中的配置逐字段相同。生成号**不动**。
    Unchanged,
    /// 含冷段变更，整批拒绝，last-good 原封不动。
    RejectedCold {
        /// 待重启清单。
        pending_restart: Vec<&'static str>,
    },
    /// 读不出来、太大、或者管线不认。last-good 原封不动。
    Failed(BootstrapError),
}

impl ReloadOutcome {
    /// 用例失败信息里用的短名。
    ///
    /// `#[cfg(test)]`：四条日志记录各自写死了自己的 `name:`（见 [`Self::reload`]），没有
    /// 一处需要在运行期把分支名算出来。挂在这儿只给用例用——`panic!("实际是 {}", …)`
    /// 比 `assert!(matches!(…))` 失败时能说出到底走了哪一支。
    ///
    /// [`Self::reload`]: ReloadSource::reload
    #[cfg(test)]
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Applied { .. } => "applied",
            Self::Unchanged => "unchanged",
            Self::RejectedCold { .. } => "rejected_cold",
            Self::Failed(_) => "failed",
        }
    }
}

impl ReloadSource {
    /// 从启动结果里把重载要用的东西留下来。
    ///
    /// 参数是 `path` 与 `source` 而不是整个 `Loaded`：`Loaded.config` 要被装配层拿走，
    /// 把它整个存进来就得多一次克隆，而这里一个字段都用不上它。
    pub(crate) fn new(
        path: PathBuf,
        source: PathSource,
        env: &ProcessEnv,
        install_root: PathBuf,
    ) -> Self {
        Self {
            path,
            source,
            vars: bootstrap::utf8_vars(&env.vars),
            install_root,
        }
    }

    /// 跑一次重载。
    ///
    /// 走的是与启动**同一条**管线的后半程（[`bootstrap::parse`]），前半程（选路径、
    /// 必要时落模板）故意不共享——理由见模块文档。
    ///
    /// [`bootstrap::parse`]: super::bootstrap::parse
    pub(crate) fn reload(&self, publisher: &mut ConfigPublisher) -> ReloadOutcome {
        let text = match bootstrap::read_limited(&self.path) {
            Ok(text) => text,
            Err(err) => {
                return Self::failed(bootstrap::classify_read(&self.path, &self.source, err));
            }
        };

        let (next, clamps) = match bootstrap::parse(&text, &self.vars, &self.install_root) {
            Ok(parsed) => parsed,
            Err(err) => return Self::failed(BootstrapError::Config(err)),
        };

        // 判决在一个内层块里做完：`publisher.current()` 借着 publisher，而下面的
        // `publish` 要 `&mut`。让快照在这里就析构，比整个函数拖着一份 `Arc<Config>` 干净。
        // `ReloadDecision` 不从快照借任何东西（`Accepted` 自己拥有 `Box<Config>`，
        // `HeatDiff` 里全是 `&'static str`），所以它出得来。
        let decision = {
            let active = publisher.current();
            evaluate_reload(active.config(), next)
        };

        match decision {
            ReloadDecision::RejectCold { diff } => {
                let pending_restart = diff.cold_fields().to_vec();
                // 半热与热字段也一并报出来：它们这一次**没有**生效。不说清楚的话，
                // 运维会以为"拒绝的只是冷的那几个"。
                warn!(
                    name: "config_reload_rejected",
                    pending_restart = ?pending_restart,
                    also_discarded_semi = ?diff.semi_fields(),
                    also_discarded_hot = ?diff.hot_fields(),
                    "config reload rejected: cold fields changed, restart required"
                );
                ReloadOutcome::RejectedCold { pending_restart }
            }
            // 空 diff：新旧逐字段相同。这里省掉 `publish`，是为了不让生成号因为一次
            // 无意义的 SIGHUP 前进——生成号一动，每个读端都会认为"配置变了"而重新取值。
            ReloadDecision::Accept { diff, .. } if diff.is_empty() => {
                info!(
                    name: "config_reload_unchanged",
                    generation = publisher.current().generation().get(),
                    "config reload found no change"
                );
                ReloadOutcome::Unchanged
            }
            ReloadDecision::Accept { config, diff } => {
                let hot = diff.hot_fields().to_vec();
                let semi = diff.semi_fields().to_vec();
                let generation = publisher.publish(config);
                // 钳位并进这一条记录，不另发 `config_clamped`：同一件事记两处会让运维
                // 以为发生了两次（`ClampRecord` 的文档里写的就是这条）。只有真的换上去了
                // 才报——被拒绝的那一批里的钳位从未生效，报出来是噪声。
                info!(
                    name: "config_reloaded",
                    generation = generation.get(),
                    hot = ?hot,
                    // 半热单列：它们已经进了真值，但要等对应的面重启才看得见效果。
                    // 合进 `hot` 会让人以为立刻生效了。
                    semi_pending_plane_restart = ?semi,
                    clamped = ?clamps.iter().map(|c| c.field.as_str()).collect::<Vec<_>>(),
                    "config reloaded"
                );
                ReloadOutcome::Applied {
                    generation,
                    hot,
                    semi,
                    clamps,
                }
            }
        }
    }

    /// 失败的公共出口：记一条，然后把错误原样带出去。
    ///
    /// 单独一个函数是为了让"失败只记一条日志"成为结构性的——两个失败分支都只能从这里出去。
    fn failed(err: BootstrapError) -> ReloadOutcome {
        error!(
            name: "config_reload_failed",
            error = %err,
            "config reload failed; the running config is unchanged"
        );
        ReloadOutcome::Failed(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;
    use std::time::Duration;

    use service_testkit::TempInstallRoot;

    /// 一份最小但完整的配置文本。用例改它的某一行来制造各档变更。
    ///
    /// 不用 [`bootstrap::EMBEDDED_TEMPLATE`]：那一份里大多数键是注释掉的，改一个值得先
    /// 取消注释，用例读起来就全是与判据无关的噪声。
    const BASE: &str = "\
[http]
bind_addr = \"127.0.0.1:8080\"
handler_timeout = \"15s\"
body_limit_bytes = 1048576

[worker]
enabled = true
tick_interval = \"30s\"
";

    fn env_in(root: &TempInstallRoot) -> ProcessEnv {
        ProcessEnv {
            exe_path: root.exe_path().to_path_buf(),
            args: vec![OsString::from("service")],
            vars: BTreeMap::new(),
            stderr_is_terminal: false,
            bin_name: "service",
        }
    }

    /// 摆好「已经启动完成」的现场：配置文件在磁盘上，publisher 装着从它解出来的那一份。
    ///
    /// 用例因此验的是"从 last-good 出发再走一次"，而不是"从 `Config::default` 出发"——
    /// 后者会让"生成号有没有动"这类判据失去意义。
    fn running(body: &str) -> (TempInstallRoot, ReloadSource, ConfigPublisher) {
        let root = TempInstallRoot::new().unwrap();
        let path = root.write_config(body).unwrap();
        let env = env_in(&root);
        let (config, _) =
            bootstrap::parse(body, &bootstrap::utf8_vars(&env.vars), root.root()).unwrap();
        let (publisher, _reader) = ConfigPublisher::new(config);
        let source = ReloadSource::new(path, PathSource::Default, &env, root.root().to_path_buf());
        (root, source, publisher)
    }

    /// 具名证据：文件没了就是失败，生效中的那一份一个字段都不许动。
    #[test]
    fn reload_with_missing_file_fails_and_keeps_last_good() {
        let (_root, source, mut publisher) = running(BASE);
        let before = publisher.current();

        std::fs::remove_file(&source.path).unwrap();
        let outcome = source.reload(&mut publisher);

        assert!(
            matches!(outcome, ReloadOutcome::Failed(BootstrapError::Read { .. })),
            "文件缺失该报读失败，实际是 {}",
            outcome.as_str()
        );

        let after = publisher.current();
        assert_eq!(
            after.generation(),
            before.generation(),
            "失败的重载不许推进生成号"
        );
        assert_eq!(after.config(), before.config(), "last-good 必须原封不动");
    }

    /// 上一条的另一半：失败之后磁盘上也不许凭空长出一份缺省配置。
    ///
    /// 与上一条分开，是因为它们坏的方式不一样：上一条坏在"报告了成功"，这一条坏在
    /// "运维的文件被换成了出厂值"。后者即使前者修好了也可能单独发生。
    #[test]
    fn reload_never_recreates_a_deleted_config_file() {
        let (_root, source, mut publisher) = running(BASE);
        std::fs::remove_file(&source.path).unwrap();

        let _ = source.reload(&mut publisher);

        assert!(
            !source.path.exists(),
            "重载把配置文件重建出来了——`materialize_default` 漏到了重载路径上"
        );
    }

    /// 改热字段：换值 + 生成号前进。
    #[test]
    fn a_hot_change_is_applied_and_advances_the_generation() {
        let (_root, source, mut publisher) = running(BASE);
        let before = publisher.current().generation();

        let next = BASE.replace("tick_interval = \"30s\"", "tick_interval = \"5s\"");
        std::fs::write(&source.path, next).unwrap();

        let outcome = source.reload(&mut publisher);
        let ReloadOutcome::Applied {
            generation,
            hot,
            semi,
            clamps,
        } = outcome
        else {
            panic!("热字段该被接受，实际是 {}", outcome.as_str());
        };

        assert_eq!(hot, ["worker.tick_interval"]);
        assert!(semi.is_empty());
        assert!(clamps.is_empty(), "这一批没有该被钳位的值");
        assert!(generation.get() > before.get(), "换了值就该推进生成号");
        assert_eq!(
            publisher.current().config().worker.tick_interval,
            Duration::from_secs(5),
            "真值必须已经是新的那一份"
        );
    }

    /// 「整批拒绝」：一个冷字段就足以把同一批里的热字段一起挡在外面。
    ///
    /// 这条用例故意**同时**改冷的和热的——只改冷的验不出"整批"这两个字。
    #[test]
    fn a_cold_change_rejects_the_whole_batch() {
        let (_root, source, mut publisher) = running(BASE);
        let before = publisher.current();

        let next = BASE
            .replace(
                "bind_addr = \"127.0.0.1:8080\"",
                "bind_addr = \"127.0.0.1:9090\"",
            )
            .replace("tick_interval = \"30s\"", "tick_interval = \"5s\"");
        std::fs::write(&source.path, next).unwrap();

        let outcome = source.reload(&mut publisher);
        let ReloadOutcome::RejectedCold { pending_restart } = outcome else {
            panic!("含冷段变更该被拒绝，实际是 {}", outcome.as_str());
        };
        assert_eq!(pending_restart, ["http.bind_addr"]);

        let after = publisher.current();
        assert_eq!(after.generation(), before.generation());
        assert_eq!(
            after.config().worker.tick_interval,
            Duration::from_secs(30),
            "同一批里的热字段也不许生效——这就是「整批」"
        );
    }

    /// 内容没变的 SIGHUP 不推生成号：生成号一动，每个读端都会重新取值。
    #[test]
    fn an_identical_file_does_not_advance_the_generation() {
        let (_root, source, mut publisher) = running(BASE);
        let before = publisher.current().generation();

        let outcome = source.reload(&mut publisher);

        assert!(
            matches!(outcome, ReloadOutcome::Unchanged),
            "同一份内容该报无变化，实际是 {}",
            outcome.as_str()
        );
        assert_eq!(publisher.current().generation(), before);
    }

    /// 语法坏掉的文件同样保留 last-good——失败的形态不止"文件没了"这一种。
    #[test]
    fn a_malformed_file_keeps_last_good() {
        let (_root, source, mut publisher) = running(BASE);
        let before = publisher.current();

        std::fs::write(&source.path, "[http\nbind_addr = ").unwrap();
        let outcome = source.reload(&mut publisher);

        assert!(
            matches!(outcome, ReloadOutcome::Failed(BootstrapError::Config(_))),
            "语法错该报管线失败，实际是 {}",
            outcome.as_str()
        );
        assert_eq!(publisher.current().config(), before.config());
    }

    /// 钳位记录要跟着重载报告一起出来，而不是被吞掉。
    #[test]
    fn a_clamped_value_is_reported_with_the_reload() {
        let (_root, source, mut publisher) = running(BASE);

        // 0 会让"超时"退化成每个请求立即失败，管线把它钳到下界。
        let next = BASE.replace("handler_timeout = \"15s\"", "handler_timeout = \"0s\"");
        std::fs::write(&source.path, next).unwrap();

        let outcome = source.reload(&mut publisher);
        let ReloadOutcome::Applied { clamps, .. } = outcome else {
            panic!("钳位不是失败，该照常换上去，实际是 {}", outcome.as_str());
        };
        assert_eq!(
            clamps.iter().map(|c| c.field.as_str()).collect::<Vec<_>>(),
            ["http.handler_timeout"],
            "钳位必须出现在重载报告里"
        );
    }

    /// `ReloadSource` 的 `Debug` 不许把环境变量的值打出来。
    ///
    /// 这不是格式洁癖：那份快照是整个进程的环境，密钥就在里面。
    #[test]
    fn debug_does_not_leak_environment_values() {
        let root = TempInstallRoot::new().unwrap();
        let mut env = env_in(&root);
        env.vars.insert(
            OsString::from("SERVICE_DB_PASSWORD"),
            OsString::from("hunter2"),
        );
        let source = ReloadSource::new(
            root.default_config_path(),
            PathSource::Default,
            &env,
            root.root().to_path_buf(),
        );

        let rendered = format!("{source:?}");
        assert!(!rendered.contains("hunter2"), "环境变量的值漏进了 Debug");
        assert!(
            rendered.contains("<1 entries>"),
            "条数还是要报的，否则排查不了「快照是不是空的」：{rendered}"
        );
    }
}
