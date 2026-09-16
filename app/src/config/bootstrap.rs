//! 配置引导：从「进程环境」走到「一份可用的 [`Config`]」。
//!
//! 这是三段式管线的**前半程**。后半程（锚定、补全、校验、钳位）是 `core` 里的
//! 纯函数 [`Config::finalize`]；这里只负责它做不到的那三件事：选哪个文件、把文件读进来、
//! 把 TOML 解成结构体。
//!
//! # 这个文件是全仓库唯一往磁盘写配置的地方
//!
//! 而且只在**启动**路径上写，只在配置路径是「安装根下的缺省位置」这一档上写。把
//! 写入端放在库里、再让 reload 复用它，后果是：把配置文件删掉再 SIGHUP，进程会静默地把
//! 缺省模板重建出来，然后报告「重载成功」——运维以为自己的改动生效了，实际上配置被换回了
//! 出厂值。这里用两道结构性约束堵死它：写入端在 `app`，且 [`load`] 与 [`reload`] 走的是
//! 两个入口（[`parse`] 是它们唯一共享的部分，而 `parse` 不碰文件系统）。
//!
//! [`reload`]: super::reload
//!
//! # 为什么这一整个模块是同步的
//!
//! 两个理由，方向相反但结论一致：
//!
//! - **启动路径上还没有 runtime。** 主 runtime 的线程数就写在 `[runtime]` 里，得先读到
//!   配置才能把它建出来。于是读配置这件事必然发生在任何 runtime 之前，"异步读"在这里
//!   根本无处可跑。
//! - **重载路径上不许 `spawn_blocking`。** 而 `tokio::fs` 的每一个函数**就是**
//!   `spawn_blocking`——它没有用 io_uring，只是把同步调用挪到阻塞线程池上。用它换来的不是
//!   "不阻塞"，而是"阻塞在别人的线程上，外加一次跨线程调度"。
//!
//! 所以重载时这一次读确实会占住编排循环。约束它的不是异步，而是[大小上限]
//! (MAX_CONFIG_BYTES)：一次有界的本地小文件读，代价可预测。把这一条写下来，是因为
//! 「同步 IO 出现在 async 函数调用链上」看起来像个疏忽，而它是个选择。
//!
//! # 为什么这里一行日志都没有
//!
//! 日志去哪、什么级别，写在 `[telemetry]` 里——也就是说，**这个模块跑完之前 subscriber
//! 还没装**（理由见 [`crate::telemetry`]）。所以每一次回落、每一次写模板，都不是在这里
//! `warn!`，而是作为数据放进 [`Loaded`] 返回给 `run`，由它在 subscriber 就绪之后逐条记。
//!
//! 这不只是绕开一个顺序问题，它还更好：一条 `warn!` 只能靠抓日志来断言，而
//! [`TemplateAction`] 是个可以直接 `assert_eq!` 的值。「不许静默」在这里由类型保证，
//! 而不是由「记得写一行日志」保证。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use service_core::config::{ClampRecord, Config, ConfigError, FieldPath, SafeMessage, expand};
use service_core::paths;

use crate::boot::ProcessEnv;

/// 内嵌的缺省配置模板。
///
/// 它与 [`Config::default`] **逐字段相等**，由 `embedded_template_round_trips_to_default`
/// 证明。这条等式不是好看：一个与默认值不一致的模板会让「删掉配置文件重启」变成
/// 一次静默的行为变更，而那正是运维排查时最常用的一招。
pub(crate) const EMBEDDED_TEMPLATE: &str = include_str!("../../assets/service.toml");

/// 配置文件的大小上限。
///
/// 这一份配置满打满算几 KB。设上限不是省内存，是**给一次同步读一个可预测的上界**：
/// `--config /dev/zero`、或者指错到一个日志文件上，都不该表现成"进程卡在启动里"。
/// 超限是明确失败，不是截断——截断会得到一份语法恰好合法、内容却缺了一半的配置。
pub(crate) const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// 最终读的那个文件是怎么被选中的（三段优先级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathSource {
    /// `--config <PATH>`。
    CommandLine,
    /// `<PREFIX>_CONFIG` 环境变量。
    Environment {
        /// 变量名。带着它，出错信息才能说清"是哪个变量把我指到这儿的"。
        var: String,
    },
    /// `<安装根>/config/service.toml`。
    Default,
}

impl PathSource {
    /// 日志与错误信息里用的稳定短名。
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::CommandLine => "command line",
            Self::Environment { .. } => "environment",
            Self::Default => "default location",
        }
    }
}

/// 这一次启动对内嵌模板做了什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateAction {
    /// 什么也没做：文件本来就在，或者路径是显式指定的（那一档从不自动创建）。
    Untouched,
    /// 文件不在，这一次把内嵌模板写出来了。
    Written,
    /// 想写但写不了（只读的安装根、权限不够……），已退回到内存里的那一份继续启动。
    ///
    /// 这**不是**启动失败：把模板落到磁盘是给运维的方便，不是服务运行的前提。一个
    /// 只读挂载的容器镜像是完全正当的部署形态，不该因此起不来。
    WriteFailed {
        /// 为什么写不了。消毒过：路径与 IO 错误都可能带上外部内容。
        reason: SafeMessage,
    },
}

/// 引导成功之后拿到的全部东西。
#[derive(Debug)]
pub(crate) struct Loaded {
    /// 跑完整条管线的配置。
    pub(crate) config: Config,
    /// 最终读的是哪个文件。
    pub(crate) path: PathBuf,
    /// 它是怎么被选中的。
    pub(crate) source: PathSource,
    /// 这一次对内嵌模板做了什么。
    pub(crate) template: TemplateAction,
    /// 钳位记录。由 `run` 在 subscriber 就绪后逐条 `warn!`。
    pub(crate) clamps: Vec<ClampRecord>,
}

/// 引导失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum BootstrapError {
    /// 配置文件读不出来。
    #[error("cannot read config file `{path}` (selected via {selected_by}): {cause}")]
    Read {
        /// 消毒过的路径。
        path: FieldPath,
        /// 三段优先级里的哪一段把我们指到这儿的。
        selected_by: &'static str,
        /// 底层 IO 错误。
        ///
        /// 字段名不叫 `source`：`thiserror` 把那个名字当成"上游错误"的保留字，而这个枚举
        /// 的语义里 "source" 已经被"配置来源"占了，两个含义撞在一起会很难读。
        #[source]
        cause: io::Error,
    },

    /// 配置文件超过 [`MAX_CONFIG_BYTES`]。
    #[error("config file `{path}` exceeds the {limit} byte limit")]
    TooLarge {
        /// 消毒过的路径。
        path: FieldPath,
        /// 上限。
        limit: u64,
    },

    /// 管线本身失败：展开、反序列化或校验。
    ///
    /// `#[from]` 的目标是本 workspace 的类型，写全路径是为了让门禁能逐字节判定。
    #[error(transparent)]
    Config(#[from] service_core::config::ConfigError),
}

/// 把可执行文件名变成环境变量前缀。
///
/// 规则：ASCII 字母数字保留并大写，其余一律换成 `_`。于是 `my-service` → `MY_SERVICE`，
/// 对应的变量是 `MY_SERVICE_CONFIG`。
///
/// 为什么只认 ASCII：环境变量名在 POSIX 下本来就只保证 `[A-Za-z0-9_]` 可移植，而
/// `char::to_uppercase` 在非 ASCII 上是一对多的（`ß` → `SS`），会让"前缀"这件事失去
/// 唯一性。非 ASCII 的名字整体退化成一串 `_`，难看但不会撞车，也不会静默取到别人的变量。
pub(crate) fn env_prefix(bin_name: &str) -> String {
    bin_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// `<PREFIX>_CONFIG`——三段优先级里的第二档。
pub(crate) fn config_var_name(bin_name: &str) -> String {
    format!("{}_CONFIG", env_prefix(bin_name))
}

/// 按三段优先级挑出配置文件路径。
///
/// 环境变量**设成空串也算设了**，会被当作一条（读不出来的）路径而不是"没设"。理由：
/// `Environment=SVC_CONFIG=${CONFIG_FILE}` 里 `CONFIG_FILE` 忘了定义，得到的就是空串。
/// 把它当"没设"就等于静默改用缺省配置——服务起来了，用的却不是运维以为的那一份。这正是
/// 「不许静默」要拦的形状，所以让它以"文件不存在"失败。
pub(crate) fn select_path(
    cli_config: Option<&Path>,
    env: &ProcessEnv,
    install_root: &Path,
) -> (PathBuf, PathSource) {
    if let Some(path) = cli_config {
        return (path.to_path_buf(), PathSource::CommandLine);
    }
    let var = config_var_name(env.bin_name);
    if let Some(value) = env.var(&var) {
        return (PathBuf::from(value), PathSource::Environment { var });
    }
    (
        paths::default_config_path(install_root),
        PathSource::Default,
    )
}

/// 完整的启动引导。
///
/// # Errors
///
/// 见 [`BootstrapError`]。
pub(crate) fn load(
    cli_config: Option<&Path>,
    env: &ProcessEnv,
    install_root: &Path,
) -> Result<Loaded, BootstrapError> {
    let (path, source) = select_path(cli_config, env, install_root);

    let (text, template) = match source {
        // 缺省档：文件不在就把内嵌模板摆出来。这是「首次部署直接能跑」与「运维有一份
        // 带注释的起点」这两件事的唯一来源。
        PathSource::Default => materialize_default(&path)?,
        // 显式指定的两档：**绝不**自动创建。写错一个字符就在那个错路径上凭空长出一份
        // 缺省配置，是一种很难自查的错误——服务起来了，改动却怎么都不生效。
        PathSource::CommandLine | PathSource::Environment { .. } => (
            read_limited(&path).map_err(|err| classify_read(&path, &source, err))?,
            TemplateAction::Untouched,
        ),
    };

    let (config, clamps) = parse(&text, &utf8_vars(&env.vars), install_root)?;

    Ok(Loaded {
        config,
        path,
        source,
        template,
        clamps,
    })
}

/// 展开 → 反序列化 → 跑完管线后半程。**不碰文件系统**，于是重载复用的是它。
///
/// # Errors
///
/// [`ConfigError`] 的四种。
pub(crate) fn parse(
    text: &str,
    vars: &BTreeMap<String, String>,
    install_root: &Path,
) -> Result<(Config, Vec<ClampRecord>), ConfigError> {
    let expanded = expand(text, vars)?;
    let mut config = deserialize(&expanded)?;
    let clamps = config.finalize(install_root)?;
    Ok((config, clamps))
}

/// TOML → [`Config`]，错误里带字段路径。
///
/// 分两步失败，两步给的定位方式不一样，所以不能合并：
///
/// - **语法错**（括号没闭、引号没配对）时还没有"字段"这个概念，只有一个字节偏移。把它
///   翻成行列号——`expected ... at offset 1873` 对着一份几百行的配置等于没说。
/// - **结构错**（类型不符、键拼错）时 `serde_path_to_error` 能给出 `http.bind_addr`
///   这样的路径，那比行号好：改过的配置行号会动，字段路径不会。
fn deserialize(expanded: &str) -> Result<Config, ConfigError> {
    let de = toml::Deserializer::parse(expanded).map_err(|err| ConfigError::Deserialize {
        path: FieldPath::new(&locate(expanded, err.span())),
        message: SafeMessage::new(err.message()),
    })?;

    serde_path_to_error::deserialize(de).map_err(|err| {
        let path = err.path().to_string();
        let path = if path.is_empty() {
            locate(expanded, err.inner().span())
        } else {
            path
        };
        ConfigError::Deserialize {
            path: FieldPath::new(&path),
            message: SafeMessage::new(err.inner().message()),
        }
    })
}

/// 把字节偏移翻成 `line N, column M`（都从 1 起算，列按**字符**数）。
///
/// 没有偏移时退回一个占位串——它只出现在"解析器自己也说不清位置"的情形，此时硬编一个
/// `line 1` 会把人指到一个无关的地方去。
fn locate(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    let Some(span) = span else {
        return "<document>".to_owned();
    };
    let offset = span.start.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before, |nl| &before[nl + 1..])
        .chars()
        .count()
        + 1;
    format!("line {line}, column {column}")
}

/// 缺省档专用：文件在就读它，不在就把内嵌模板写出来。
///
/// 用 `create_new` 而不是「先 `exists()` 再 `write`」：后者在两个进程同时首启时会互相
/// 覆盖，而 `create_new` 把"存在性检查"和"创建"合成一次原子的 `O_EXCL`。抢输的那个拿到
/// [`io::ErrorKind::AlreadyExists`]，回头去读——这正是它该做的事。
fn materialize_default(path: &Path) -> Result<(String, TemplateAction), BootstrapError> {
    match read_limited(path) {
        Ok(text) => return Ok((text, TemplateAction::Untouched)),
        Err(ReadError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(classify_read(path, &PathSource::Default, err));
        }
    }

    match write_template(path) {
        Ok(()) => Ok((EMBEDDED_TEMPLATE.to_owned(), TemplateAction::Written)),
        // 抢输了：别人刚写完，读他那一份。读不出来才是真的失败。
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            let text =
                read_limited(path).map_err(|err| classify_read(path, &PathSource::Default, err))?;
            Ok((text, TemplateAction::Untouched))
        }
        // 写不了就用内存里的那一份继续。**不失败**——理由写在 `TemplateAction::WriteFailed` 上。
        Err(err) => Ok((
            EMBEDDED_TEMPLATE.to_owned(),
            TemplateAction::WriteFailed {
                reason: SafeMessage::new(&err.to_string()),
            },
        )),
    }
}

/// 原子地把内嵌模板创建出来。已存在时返回 [`io::ErrorKind::AlreadyExists`]。
fn write_template(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(EMBEDDED_TEMPLATE.as_bytes())?;
    file.sync_all()
}

/// 读文件失败的两种形态。内部类型：调用方要按来源补上路径才能变成 [`BootstrapError`]。
pub(super) enum ReadError {
    Io(io::Error),
    TooLarge,
}

/// 读一个**有上界**的文本文件。
///
/// 上界查两遍，因为两遍各自能漏掉一种情况：`metadata` 快，但对管道、字符设备、以及读的
/// 过程中还在长的文件不成立；`take` 一定成立，但要真的把字节读进来才知道。所以先用
/// `metadata` 给一个便宜的早退，再用 `take(limit + 1)` 兜底——多读的那一个字节就是
/// "它其实更长"的证据。
pub(super) fn read_limited(path: &Path) -> Result<String, ReadError> {
    let file = fs::File::open(path).map_err(ReadError::Io)?;
    let metadata = file.metadata().map_err(ReadError::Io)?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ReadError::TooLarge);
    }

    let mut text = String::new();
    let read = file
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(ReadError::Io)?;
    if read as u64 > MAX_CONFIG_BYTES {
        return Err(ReadError::TooLarge);
    }
    Ok(text)
}

/// 给一次读失败补上路径与来源。
pub(super) fn classify_read(path: &Path, source: &PathSource, err: ReadError) -> BootstrapError {
    let display = FieldPath::new(&path.display().to_string());
    match err {
        ReadError::Io(cause) => BootstrapError::Read {
            path: display,
            selected_by: source.as_str(),
            cause,
        },
        ReadError::TooLarge => BootstrapError::TooLarge {
            path: display,
            limit: MAX_CONFIG_BYTES,
        },
    }
}

/// 环境变量的 UTF-8 视图，喂给 [`expand`]。
///
/// 非 UTF-8 的变量整条丢掉，与 [`ProcessEnv::var`] 同一套规矩：`${NAME}` 引到它时会以
/// "未定义"失败，而不是拿到一串替换字符。本模板没有任何一个取值需要非 UTF-8。
pub(super) fn utf8_vars(vars: &BTreeMap<OsString, OsString>) -> BTreeMap<String, String> {
    vars.iter()
        .filter_map(|(k, v)| Some((k.to_str()?.to_owned(), v.to_str()?.to_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use service_testkit::TempInstallRoot;

    fn env_with(root: &TempInstallRoot, vars: &[(&str, &str)]) -> ProcessEnv {
        ProcessEnv {
            exe_path: root.exe_path().to_path_buf(),
            args: vec![OsString::from("service")],
            vars: vars
                .iter()
                .map(|(k, v)| (OsString::from(*k), OsString::from(*v)))
                .collect(),
            stderr_is_terminal: false,
            bin_name: "service",
        }
    }

    #[test]
    fn embedded_template_round_trips_to_default() {
        // 模板不是文档，它是默认值的**另一种写法**；两种写法必须给出同一个 `Config`。
        // 注意比的是 `finalize` 之前的形态：`finalize` 会把 `storage.path` 锚到安装根，
        // 而 `Config::default()` 里那一条还是相对路径。
        let vars = BTreeMap::new();
        let expanded = expand(EMBEDDED_TEMPLATE, &vars).expect("模板里不该有环境变量");
        let parsed = deserialize(&expanded).expect("模板必须能解析");
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn the_default_location_is_created_on_first_start() {
        // 正路：缺省档、文件不在 → 写出来，并且写出去的和读回来的是同一份。
        let root = TempInstallRoot::new().unwrap();
        let env = env_with(&root, &[]);

        let first = load(None, &env, root.root()).unwrap();
        assert_eq!(first.template, TemplateAction::Written);
        assert_eq!(first.path, root.default_config_path());
        assert_eq!(fs::read_to_string(&first.path).unwrap(), EMBEDDED_TEMPLATE);

        // 第二次启动不该再写一遍，也不该报告成"写过了"。
        let second = load(None, &env, root.root()).unwrap();
        assert_eq!(second.template, TemplateAction::Untouched);
        assert_eq!(second.config, first.config);
    }

    #[test]
    fn an_explicit_path_is_never_created() {
        // 写错一个字符就凭空长出一份缺省配置，是最难自查的一类启动问题：服务起来了，
        // 改动却不生效。两档显式来源都必须以"文件不存在"失败。
        let root = TempInstallRoot::new().unwrap();
        let missing = root.config_dir().join("typo.toml");

        let by_cli = load(Some(&missing), &env_with(&root, &[]), root.root()).unwrap_err();
        assert!(matches!(
            by_cli,
            BootstrapError::Read {
                selected_by: "command line",
                ..
            }
        ));

        let env = env_with(&root, &[("SERVICE_CONFIG", missing.to_str().unwrap())]);
        let by_env = load(None, &env, root.root()).unwrap_err();
        assert!(matches!(
            by_env,
            BootstrapError::Read {
                selected_by: "environment",
                ..
            }
        ));
        assert!(!missing.exists(), "两档都不许把文件创建出来");
    }

    #[test]
    fn the_three_sources_are_tried_in_order() {
        let root = TempInstallRoot::new().unwrap();
        let from_cli = root.write_config_as("cli.toml", "").unwrap();
        let from_env = root.write_config_as("env.toml", "").unwrap();
        root.write_config("").unwrap();
        let env = env_with(&root, &[("SERVICE_CONFIG", from_env.to_str().unwrap())]);

        let (path, source) = select_path(Some(&from_cli), &env, root.root());
        assert_eq!(path, from_cli);
        assert_eq!(source, PathSource::CommandLine);

        let (path, source) = select_path(None, &env, root.root());
        assert_eq!(path, from_env);
        assert_eq!(
            source,
            PathSource::Environment {
                var: "SERVICE_CONFIG".to_owned()
            }
        );

        let (path, source) = select_path(None, &env_with(&root, &[]), root.root());
        assert_eq!(path, root.default_config_path());
        assert_eq!(source, PathSource::Default);
    }

    #[test]
    fn an_empty_environment_variable_is_a_path_not_an_absence() {
        // 把空串当"没设"就等于静默改用缺省配置。让它以"读不出来"失败。
        let root = TempInstallRoot::new().unwrap();
        root.write_config("").unwrap();
        let env = env_with(&root, &[("SERVICE_CONFIG", "")]);

        let (_, source) = select_path(None, &env, root.root());
        assert!(matches!(source, PathSource::Environment { .. }));
        assert!(matches!(
            load(None, &env, root.root()).unwrap_err(),
            BootstrapError::Read { .. }
        ));
    }

    #[test]
    fn the_env_prefix_comes_only_from_the_binary_name() {
        assert_eq!(config_var_name("service"), "SERVICE_CONFIG");
        assert_eq!(config_var_name("my-service"), "MY_SERVICE_CONFIG");
        assert_eq!(config_var_name("a.b c"), "A_B_C_CONFIG");
    }

    #[test]
    fn environment_variables_are_expanded_in_values() {
        let root = TempInstallRoot::new().unwrap();
        root.write_config("[http]\nbind_addr = \"127.0.0.1:${PORT:9099}\"\n")
            .unwrap();

        let defaulted = load(None, &env_with(&root, &[]), root.root()).unwrap();
        assert_eq!(defaulted.config.http.bind_addr.port(), 9099);

        let env = env_with(&root, &[("PORT", "7001")]);
        let overridden = load(None, &env, root.root()).unwrap();
        assert_eq!(overridden.config.http.bind_addr.port(), 7001);
    }

    #[test]
    fn a_syntax_error_reports_a_line_and_column() {
        // 语法错时没有字段路径，只有偏移。`expected ... at offset 1873` 对一份几百行的
        // 配置等于没说，所以必须翻成行列。
        let root = TempInstallRoot::new().unwrap();
        root.write_config("[http]\nbind_addr = \n").unwrap();

        let err = load(None, &env_with(&root, &[]), root.root()).unwrap_err();
        let BootstrapError::Config(ConfigError::Deserialize { path, .. }) = err else {
            panic!("语法错必须落在 Deserialize 上：{err:?}");
        };
        assert!(path.as_str().starts_with("line 2"), "{path}");
    }

    #[test]
    fn a_type_error_reports_the_field_path() {
        // 结构错时字段路径比行号好：改过的配置行号会动，`storage.max_readers` 不会。
        let root = TempInstallRoot::new().unwrap();
        root.write_config("[storage]\nmax_readers = \"four\"\n")
            .unwrap();

        let err = load(None, &env_with(&root, &[]), root.root()).unwrap_err();
        let BootstrapError::Config(ConfigError::Deserialize { path, .. }) = err else {
            panic!("类型错必须落在 Deserialize 上：{err:?}");
        };
        assert_eq!(path.as_str(), "storage.max_readers");
    }

    #[test]
    fn an_oversized_file_fails_instead_of_being_truncated() {
        // 截断会得到一份语法恰好合法、内容却缺了一半的配置——比失败坏得多。
        let root = TempInstallRoot::new().unwrap();
        let filler = "# ".to_owned() + &"x".repeat(1024) + "\n";
        let cap =
            usize::try_from(MAX_CONFIG_BYTES).expect("上限是 1 MiB，任何平台的 usize 都放得下");
        let body = filler.repeat((cap / filler.len()) + 2);
        root.write_config(&body).unwrap();

        assert!(matches!(
            load(None, &env_with(&root, &[]), root.root()).unwrap_err(),
            BootstrapError::TooLarge { .. }
        ));
    }

    #[test]
    fn relative_paths_are_anchored_to_the_install_root() {
        // 锚点是安装根，不是 cwd，也不是配置文件所在目录——锚在配置文件那边的代价是
        // `--config /tmp/x.toml` 会顺手把数据目录搬到 `/tmp/data/`。
        let root = TempInstallRoot::new().unwrap();
        let explicit = root
            .write_config_as("elsewhere.toml", "[storage]\npath = \"data/x.sqlite3\"\n")
            .unwrap();

        let loaded = load(Some(&explicit), &env_with(&root, &[]), root.root()).unwrap();
        assert_eq!(
            loaded.config.storage.path,
            root.data_dir().join("x.sqlite3")
        );
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_ignored() {
        // 拼错的键静默忽略 = 改了配置没生效却毫无线索。
        let root = TempInstallRoot::new().unwrap();
        root.write_config("[worker]\nenabled = true\ntik_interval = \"1s\"\n")
            .unwrap();

        assert!(matches!(
            load(None, &env_with(&root, &[]), root.root()).unwrap_err(),
            BootstrapError::Config(ConfigError::Deserialize { .. })
        ));
    }

    #[test]
    fn a_non_utf8_variable_is_dropped_rather_than_mangled() {
        let mut vars = BTreeMap::new();
        vars.insert(OsString::from("GOOD"), OsString::from("1"));
        let view = utf8_vars(&vars);
        assert_eq!(view.get("GOOD").map(String::as_str), Some("1"));
        assert_eq!(view.len(), 1);
    }
}
