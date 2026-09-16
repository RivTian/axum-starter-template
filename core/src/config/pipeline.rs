//! 三段式管线的后半程：`resolve_paths` → `complete` → `validate` → `clamp`。
//!
//! 前半程（选路径、读文件、展开、反序列化）在 `app`——那几步都要碰进程边界。从
//! `resolve_paths` 开始全是纯函数，因此全都在这里，也因此全都可测。
//!
//! # 顺序由类型保证
//!
//! 这四步有严格顺序：路径没锚定就校验，会把一个合法的相对路径判成非法；钳位跑在校验
//! 之前，会把本该拦下的配置钳成"能跑但不对"。
//!
//! 所以对外只给一个 [`Config::finalize`]，四步在它里面按顺序跑完。调用方**没有**把顺序
//! 写错的机会——启动路径和重载路径调的是同一个函数，"共用同一条管线"因此是结构事实，
//! 不是需要两处各自维护的约定。
//!
//! # 校验与钳位的分界
//!
//! - [`validate`](Config::validate)：只拦**继续跑必然失败**的。拦下来就是起不来。
//! - [`clamp`](Config::clamp)：其余越界的一律夹回合法区间，并留下记录。
//!
//! 分界线画在这里的理由：一个配错的数字不该让服务起不来，但也不该被静默接受。钳位记录
//! 让运维在日志里看到"你写的 0 被当成 1 用了"，而服务照常提供能力。

use std::path::Path;
use std::time::Duration;

use super::error::{ConfigError, FieldPath};
use super::types::{Config, MAIN_RUNTIME_NAME, RuntimeThreads};
use crate::paths;

/// 请求体上限的下界。0 会让服务拒绝一切带 body 的请求，那等于没在提供能力。
const MIN_BODY_LIMIT_BYTES: usize = 1024;
/// 时长类旋钮的下界。0 会让"超时"和"间隔"退化成忙等或立即失败。
const MIN_DURATION: Duration = Duration::from_millis(1);

/// 一次钳位。
///
/// 故意**不**在 `core` 里打日志：`app` 在启动路径上把它们逐条 `warn!`，在重载路径上把它们
/// 并进重载报告。同一件事记两处会让运维以为发生了两次。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClampRecord {
    /// 被钳位的字段路径。
    ///
    /// 不是 `&'static str`：附加 runtime 的路径里含一节用户起的名字
    /// （`runtime.extra.io.worker_threads`），编译期拼不出来。[`FieldPath`] 在构造时消毒，
    /// 于是"名字里塞一个换行伪造一条日志"这条路被类型堵死，而不是靠这里记得处理。
    pub field: FieldPath,
    /// 为什么钳它。`&'static str`：配置取值进不来。
    pub reason: &'static str,
}

impl Config {
    /// 跑完管线的后半程。
    ///
    /// # Errors
    ///
    /// [`validate`](Self::validate) 认定"继续跑必然失败"时返回 [`ConfigError::Invalid`]。
    pub fn finalize(&mut self, install_root: &Path) -> Result<Vec<ClampRecord>, ConfigError> {
        self.resolve_paths(install_root);
        self.complete();
        self.validate()?;
        Ok(self.clamp())
    }

    /// 把相对路径锚到安装根。
    ///
    /// 锚点是**可执行文件目录**，不是配置文件目录，也不是 cwd——理由见 [`crate::paths`]。
    /// 空路径原样留着，交给 [`validate`](Self::validate) 去拒绝。锚定一个空路径会得到安装根
    /// 本身（`join("")` 的语义），于是 `path = ""` 会静默变成"数据库文件就是安装根那个目录"，
    /// 而 `validate` 看到的 `/opt/svc` 有文件名、完全合法。**空的东西没有可锚定之处**，
    /// 这不是校验，是锚定本身的边界。
    pub fn resolve_paths(&mut self, install_root: &Path) {
        if self.storage.path.as_os_str().is_empty() {
            return;
        }
        self.storage.path = paths::resolve_against_root(install_root, &self.storage.path);
    }

    /// 补全与归一化。
    ///
    /// 目前只有一件事：剪掉 `telemetry.filter` 两端的空白。这不是洁癖——`filter = " "`
    /// 会被 `EnvFilter` 当成一条空指令，表现是**所有日志消失**，而配置文件看起来完全正常。
    /// 剪完之后它变成空串，下一步的 `validate` 就能把它拦下来。
    pub fn complete(&mut self) {
        let trimmed = self.telemetry.filter.trim();
        if trimmed.len() != self.telemetry.filter.len() {
            self.telemetry.filter = trimmed.to_owned();
        }
    }

    /// 只拦「继续跑必然失败」。
    ///
    /// # Errors
    ///
    /// 返回 [`ConfigError::Invalid`]，带字段路径与一条 `&'static str` 的理由。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.storage.path.file_name().is_none() {
            // `file_name()` 返回 `None` 的**全部**情形：空路径、根（`/`）、以 `..` 结尾。
            // 三者都指向一个目录位置而不是一个文件，SQLite 打不开，而错误要到迁移阶段才冒
            // 出来，那时日志里已经堆了一串"正在启动"。在这里拦掉，运维只看到一行。
            //
            // 注意它**不**包括 `data/` 这种带尾分隔符的写法：`Path::file_name` 会把尾分隔符
            // 归一化掉，`data/` 与 `data` 同义。那种写法能不能开出文件取决于 `data` 当下是不
            // 是一个已存在的目录——那是文件系统事实，`core` 拿不到，也不该猜。它会在
            // `storage` 打开连接时失败，带着 io 错误的真实原因。
            return Err(ConfigError::Invalid {
                path: FieldPath::new("storage.path"),
                reason: "path points at a directory location, not a file",
            });
        }

        if self.telemetry.filter.is_empty() {
            return Err(ConfigError::Invalid {
                path: FieldPath::new("telemetry.filter"),
                reason: "filter directive must not be empty",
            });
        }

        self.validate_runtimes()?;

        Ok(())
    }

    /// 三条 runtime 规则。
    ///
    /// 三条都是**启动错误**而不是钳位，理由是同一个：它们全都会把"配置写错"伪装成别的东西。
    /// 回落到主 runtime 会让它变成"性能不达预期"，静默丢掉死配置会让它变成"下一个人的误解"。
    /// 而这三类配错的共同特征是**跑起来完全正常**——没有任何运行期信号能提示你写错了。
    fn validate_runtimes(&self) -> Result<(), ConfigError> {
        for name in self.runtime.extra.keys() {
            if name.is_empty() {
                return Err(ConfigError::Invalid {
                    path: FieldPath::new("runtime.extra"),
                    reason: "an extra runtime must have a name",
                });
            }
            if name == MAIN_RUNTIME_NAME {
                return Err(ConfigError::Invalid {
                    path: FieldPath::new("runtime.extra"),
                    reason: "`main` is reserved for the main runtime",
                });
            }
        }

        let bindings = self.plane_runtimes();

        // 引用了不存在的 runtime。
        for (field, bound) in bindings {
            let Some(name) = bound else { continue };
            if !self.runtime.extra.contains_key(name) {
                return Err(ConfigError::Invalid {
                    // 名字本身是**取值**，不进消息（见 `error.rs` 的纪律）。路径已经够定位了。
                    path: FieldPath::new(field),
                    reason: "names a runtime that is not declared under [runtime.extra]",
                });
            }
        }

        // 声明了却没有面用。
        for name in self.runtime.extra.keys() {
            if !bindings
                .iter()
                .any(|(_, bound)| *bound == Some(name.as_str()))
            {
                return Err(ConfigError::Invalid {
                    // 这里的名字是**键**，也就是字段路径的一节，可以进消息；`FieldPath` 会消毒它。
                    path: FieldPath::new(&format!("runtime.extra.{name}")),
                    reason: "declared but no plane runs on it",
                });
            }
        }

        Ok(())
    }

    /// 把越界但不致命的取值夹回合法区间。
    #[must_use]
    pub fn clamp(&mut self) -> Vec<ClampRecord> {
        let mut records = Vec::new();

        if self.http.body_limit_bytes < MIN_BODY_LIMIT_BYTES {
            self.http.body_limit_bytes = MIN_BODY_LIMIT_BYTES;
            records.push(ClampRecord {
                field: FieldPath::new("http.body_limit_bytes"),
                reason: "raised to the minimum accepted body size",
            });
        }

        if self.http.handler_timeout < MIN_DURATION {
            self.http.handler_timeout = MIN_DURATION;
            records.push(ClampRecord {
                field: FieldPath::new("http.handler_timeout"),
                reason: "raised to the minimum; zero would fail every request",
            });
        }

        if self.storage.max_readers == 0 {
            self.storage.max_readers = 1;
            records.push(ClampRecord {
                field: FieldPath::new("storage.max_readers"),
                reason: "raised to 1; a zero-sized pool hands out no connections",
            });
        }

        if self.worker.tick_interval < MIN_DURATION {
            self.worker.tick_interval = MIN_DURATION;
            records.push(ClampRecord {
                field: FieldPath::new("worker.tick_interval"),
                reason: "raised to the minimum; zero would spin a core",
            });
        }

        // 主与附加走**同一段**钳位代码。分两段写的话，"主的 0 被夹了、附加的没夹"是一个
        // 只在有人真的开了附加 runtime 时才会暴露的缺陷，而那时进程是直接 panic 的。
        let mut main = self.runtime.main();
        clamp_threads(&mut records, "runtime", &mut main);
        self.runtime.set_main(main);

        for (name, threads) in &mut self.runtime.extra {
            clamp_threads(&mut records, &format!("runtime.extra.{name}"), threads);
        }

        records
    }
}

/// 把一个 runtime 的两个线程旋钮夹回合法区间。
///
/// `Some(0)` 会让 Tokio 在构建 runtime 时 panic。这是"配置写错把进程打死"的典型形态，
/// 必须在到达 runtime 之前夹掉。`None`（交给 Tokio 定）是合法的，不动它。
fn clamp_threads(records: &mut Vec<ClampRecord>, prefix: &str, threads: &mut RuntimeThreads) {
    if threads.worker_threads == Some(0) {
        threads.worker_threads = Some(1);
        records.push(ClampRecord {
            field: FieldPath::new(&format!("{prefix}.worker_threads")),
            reason: "raised to 1; zero worker threads panics the runtime builder",
        });
    }

    if threads.max_blocking_threads == Some(0) {
        threads.max_blocking_threads = Some(1);
        records.push(ClampRecord {
            field: FieldPath::new(&format!("{prefix}.max_blocking_threads")),
            reason: "raised to 1; zero blocking threads panics the runtime builder",
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn relative_storage_paths_anchor_to_the_install_root() {
        let mut cfg = Config::default();
        cfg.resolve_paths(Path::new("/opt/svc"));
        assert_eq!(
            cfg.storage.path,
            PathBuf::from("/opt/svc/data/service.sqlite3")
        );
    }

    #[test]
    fn absolute_storage_paths_are_left_alone() {
        let mut cfg = Config::default();
        cfg.storage.path = PathBuf::from("/var/lib/svc/x.db");
        cfg.resolve_paths(Path::new("/opt/svc"));
        assert_eq!(cfg.storage.path, PathBuf::from("/var/lib/svc/x.db"));
    }

    #[test]
    fn resolve_paths_is_idempotent() {
        // 重载会在已经锚定过的配置上再跑一次管线。第二次不能把 `/opt/svc` 再拼一层。
        let mut cfg = Config::default();
        cfg.resolve_paths(Path::new("/opt/svc"));
        let once = cfg.storage.path.clone();
        cfg.resolve_paths(Path::new("/opt/svc"));
        assert_eq!(cfg.storage.path, once);
    }

    #[test]
    fn a_whitespace_only_filter_becomes_empty_then_invalid() {
        // 这是 `complete` 存在的理由：不剪的话，`filter = " "` 会让所有日志静默消失。
        let mut cfg = Config::default();
        cfg.telemetry.filter = String::from("   ");
        cfg.complete();
        assert_eq!(cfg.telemetry.filter, "");
        let err = cfg.validate().expect_err("空过滤器必须拦下");
        assert!(err.to_string().contains("telemetry.filter"));
    }

    #[test]
    fn validate_rejects_only_what_cannot_possibly_run() {
        // 「用例名 + 怎么改 + 期望被哪个字段拦下（`None` = 应当通过）」。
        // 起名同样是为了 `clippy::type_complexity`——三元组里嵌函数指针确实读不动。
        type Case = (&'static str, fn(&mut Config), Option<&'static str>);

        let cases: &[Case] = &[
            ("默认值可用", |_| {}, None),
            (
                "空路径",
                |c| c.storage.path = PathBuf::new(),
                Some("storage.path"),
            ),
            (
                "根目录",
                |c| c.storage.path = PathBuf::from("/"),
                Some("storage.path"),
            ),
            (
                "以 .. 结尾",
                |c| c.storage.path = PathBuf::from("/var/lib/svc/.."),
                Some("storage.path"),
            ),
            // 带尾分隔符的写法**不该**被拦：`Path::file_name` 把它归一化成 `data`，
            // 而 `data` 是不是一个已存在的目录是文件系统事实，`core` 拿不到。
            (
                "尾分隔符被归一化，不是错误",
                |c| c.storage.path = PathBuf::from("data/"),
                None,
            ),
            (
                "空过滤器",
                |c| c.telemetry.filter = String::new(),
                Some("telemetry.filter"),
            ),
            // 以下几条都越界，但服务钳位之后照常提供能力，因此**不该**被 validate 拦。
            (
                "零线程数交给钳位",
                |c| c.runtime.worker_threads = Some(0),
                None,
            ),
            ("零读连接交给钳位", |c| c.storage.max_readers = 0, None),
            (
                "零 body 上限交给钳位",
                |c| c.http.body_limit_bytes = 0,
                None,
            ),
            (
                "零 tick 间隔交给钳位",
                |c| c.worker.tick_interval = Duration::ZERO,
                None,
            ),
        ];

        for (i, (name, mutate, expected)) in cases.iter().enumerate() {
            let mut cfg = Config::default();
            mutate(&mut cfg);
            match (cfg.validate(), expected) {
                (Ok(()), None) => {}
                (Err(e), Some(path)) => assert!(
                    e.to_string().contains(path),
                    "TC{i} ({name}) 应指向 {path}，实际 {e}"
                ),
                (Ok(()), Some(path)) => panic!("TC{i} ({name}) 应被 {path} 拦下，却通过了"),
                (Err(e), None) => panic!("TC{i} ({name}) 不该被拦下: {e}"),
            }
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_and_recorded() {
        let mut cfg = Config::default();
        cfg.http.body_limit_bytes = 0;
        cfg.http.handler_timeout = Duration::ZERO;
        cfg.storage.max_readers = 0;
        cfg.worker.tick_interval = Duration::ZERO;
        cfg.runtime.worker_threads = Some(0);
        cfg.runtime.max_blocking_threads = Some(0);

        let records = cfg.clamp();
        let fields: Vec<&str> = records.iter().map(|r| r.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "http.body_limit_bytes",
                "http.handler_timeout",
                "storage.max_readers",
                "worker.tick_interval",
                "runtime.worker_threads",
                "runtime.max_blocking_threads",
            ],
            "钳位记录必须逐条给出，顺序稳定"
        );

        assert_eq!(cfg.http.body_limit_bytes, MIN_BODY_LIMIT_BYTES);
        assert_eq!(cfg.http.handler_timeout, MIN_DURATION);
        assert_eq!(cfg.storage.max_readers, 1);
        assert_eq!(cfg.worker.tick_interval, MIN_DURATION);
        assert_eq!(cfg.runtime.worker_threads, Some(1));
        assert_eq!(cfg.runtime.max_blocking_threads, Some(1));
    }

    /// 造一份"声明了一个附加 runtime，且 HTTP 面跑在上面"的合法配置。
    fn with_extra_runtime(name: &str) -> Config {
        let mut cfg = Config::default();
        cfg.runtime
            .extra
            .insert(name.to_owned(), RuntimeThreads::default());
        cfg.http.runtime = Some(name.to_owned());
        cfg
    }

    #[test]
    fn default_config_declares_no_extra_runtime() {
        // 证据：不用多 runtime 的服务，为这个能力付出的配置面代价是**零子表**。
        let cfg = Config::default();
        assert!(cfg.runtime.extra.is_empty());
        assert_eq!(
            cfg.plane_runtimes(),
            [("http.runtime", None), ("worker.runtime", None)]
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn a_plane_bound_to_a_declared_runtime_passes() {
        assert!(with_extra_runtime("io").validate().is_ok());
    }

    #[test]
    fn the_three_runtime_mistakes_are_startup_errors() {
        // 三条都必须是启动错误。共同点：它们跑起来**完全正常**，没有任何运行期信号。
        // 回落到主 runtime 会把"配置写错"伪装成"性能不达预期"，静默丢掉死配置会把它
        // 伪装成"下一个人的误解"。
        type Mutate = fn(&mut Config);
        let cases: &[(&str, Mutate, &str)] = &[
            (
                "引用了未声明的 runtime",
                |c| c.http.runtime = Some("io".to_owned()),
                "http.runtime",
            ),
            (
                "声明了却没有面用",
                |c| {
                    c.runtime
                        .extra
                        .insert("io".to_owned(), RuntimeThreads::default());
                },
                "runtime.extra.io",
            ),
            (
                "附加 runtime 占用了保留名 main",
                |c| {
                    c.runtime
                        .extra
                        .insert(MAIN_RUNTIME_NAME.to_owned(), RuntimeThreads::default());
                    c.http.runtime = Some(MAIN_RUNTIME_NAME.to_owned());
                },
                "runtime.extra",
            ),
            (
                "附加 runtime 没有名字",
                |c| {
                    c.runtime
                        .extra
                        .insert(String::new(), RuntimeThreads::default());
                    c.http.runtime = Some(String::new());
                },
                "runtime.extra",
            ),
        ];

        for (i, (name, mutate, path)) in cases.iter().enumerate() {
            let mut cfg = Config::default();
            mutate(&mut cfg);
            let err = cfg
                .validate()
                .expect_err(&format!("TC{i}（{name}）必须拦在启动期"));
            assert!(
                err.to_string().contains(path),
                "TC{i}（{name}）应指向 {path}，实际 {err}"
            );
        }
    }

    #[test]
    fn a_runtime_name_never_reaches_the_message_when_it_is_a_value() {
        // 名字写在面的 `runtime` 键里时它是**取值**，不进消息（`error.rs` 的纪律）；
        // 写成 `[runtime.extra.<name>]` 时它是**键**，也就是字段路径的一节，可以进。
        let mut cfg = Config::default();
        cfg.http.runtime = Some("s3cr3t".to_owned());
        let err = cfg.validate().expect_err("未声明的 runtime 必须被拦下");
        assert!(
            !err.to_string().contains("s3cr3t"),
            "取值泄漏进了消息: {err}"
        );

        let mut cfg = Config::default();
        cfg.runtime
            .extra
            .insert("s3cr3t".to_owned(), RuntimeThreads::default());
        let err = cfg.validate().expect_err("死配置必须被拦下");
        assert!(
            err.to_string().contains("s3cr3t"),
            "键理应出现在路径里: {err}"
        );
    }

    #[test]
    fn an_extra_runtime_gets_the_same_clamping_as_the_main_one() {
        // 分两段写的话，"主的 0 被夹了、附加的没夹"只在有人真开了附加 runtime 时暴露，
        // 而那时进程是直接 panic 的。
        let mut cfg = with_extra_runtime("io");
        cfg.runtime.worker_threads = Some(0);
        if let Some(extra) = cfg.runtime.extra.get_mut("io") {
            extra.worker_threads = Some(0);
            extra.max_blocking_threads = Some(0);
        }

        let records = cfg.clamp();
        let fields: Vec<&str> = records.iter().map(|r| r.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "runtime.worker_threads",
                "runtime.extra.io.worker_threads",
                "runtime.extra.io.max_blocking_threads",
            ],
            "主与附加必须各自逐条报告，路径要指到具体那一个"
        );
        assert_eq!(cfg.runtime.worker_threads, Some(1));
        assert_eq!(
            cfg.runtime.extra["io"],
            RuntimeThreads {
                worker_threads: Some(1),
                max_blocking_threads: Some(1),
            }
        );
    }

    #[test]
    fn a_clamp_record_path_cannot_forge_a_log_line() {
        // 附加 runtime 的名字来自配置文件，而钳位记录会进日志。名字里的换行必须在
        // 构造 `FieldPath` 时就被剥掉——靠的是类型，不是这里记得处理。
        let mut cfg = Config::default();
        cfg.runtime.extra.insert(
            "io\nlevel=ERROR message=breach".to_owned(),
            RuntimeThreads {
                worker_threads: Some(0),
                max_blocking_threads: None,
            },
        );

        let records = cfg.clamp();
        assert_eq!(records.len(), 1);
        assert!(!records[0].field.as_str().contains('\n'));
    }

    #[test]
    fn clamping_is_idempotent_and_silent_when_nothing_is_out_of_range() {
        let mut cfg = Config::default();
        assert!(cfg.clamp().is_empty(), "默认配置不该产生任何钳位记录");

        cfg.storage.max_readers = 0;
        assert_eq!(cfg.clamp().len(), 1);
        assert!(cfg.clamp().is_empty(), "钳过之后再钳不该重复报告");
    }

    #[test]
    fn none_thread_counts_survive_clamping() {
        // `None` 是"交给 Tokio 定"，是合法值。钳位不能把它变成 `Some(1)`——那会把一个
        // 8 核机器上的默认配置悄悄降成单线程。
        let mut cfg = Config::default();
        assert_eq!(cfg.runtime.worker_threads, None);
        assert!(cfg.clamp().is_empty());
        assert_eq!(cfg.runtime.worker_threads, None);
    }

    #[test]
    fn an_empty_storage_path_is_rejected_rather_than_becoming_the_install_root() {
        // 不特判的话：`join("")` 得到安装根本身，`validate` 看到 `/opt/svc` 有文件名、
        // 完全合法，于是 `path = ""` 静默变成"拿安装根那个目录当数据库文件"。
        let mut cfg = Config::default();
        cfg.storage.path = PathBuf::new();
        let err = cfg
            .finalize(Path::new("/opt/svc"))
            .expect_err("空路径必须被拒绝");
        assert!(err.to_string().contains("storage.path"));
        assert_eq!(cfg.storage.path, PathBuf::new(), "空路径不该被锚定成安装根");
    }

    #[test]
    fn finalize_runs_the_stages_in_the_only_correct_order() {
        let mut cfg = Config::default();
        cfg.storage.max_readers = 0;
        let records = cfg
            .finalize(Path::new("/opt/svc"))
            .unwrap_or_else(|e| panic!("默认配置加一个可钳位的值不该失败: {e}"));

        assert_eq!(
            cfg.storage.path,
            PathBuf::from("/opt/svc/data/service.sqlite3")
        );
        assert_eq!(records.len(), 1, "可钳位的值必须被钳，并且留下记录");
        assert_eq!(cfg.storage.max_readers, 1);
    }

    #[test]
    fn finalize_stops_at_validate_without_clamping() {
        // 校验失败时不该还去钳位：那会在一个已知无效的配置上做修补，日志里出现的
        // "已把 max_readers 钳到 1" 会让人以为服务起来了。
        let mut cfg = Config::default();
        cfg.telemetry.filter = String::new();
        cfg.storage.max_readers = 0;

        let err = cfg
            .finalize(Path::new("/opt/svc"))
            .expect_err("空过滤器必须让 finalize 失败");
        assert!(err.to_string().contains("telemetry.filter"));
        assert_eq!(cfg.storage.max_readers, 0, "校验失败后不该再钳位");
    }
}
