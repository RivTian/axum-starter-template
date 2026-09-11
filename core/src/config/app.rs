//! `AppConfig` 与统一加载管线
//!
//! 管线（启动与热重载共用同一条）：
//!
//! ```text
//! 读文件（缺文件 → 落盘内嵌模板再读）
//!   → ${env.NAME:default} 占位符展开
//!   → 反序列化（serde_path_to_error 定位字段路径）
//!   → resolve_paths（相对路径一律相对安装根；首个路径字段随存储段进场）
//!   → validate（硬错误，教学式信息）
//!   → sanitize（软越界钳位 + warn）
//! ```
//!
//! 校验的两层分工（字段落地时遵循）：**validate 只拦「继续跑必然失败」的
//! 取值**（反正运行时也会失败，提前到加载期错误信息更好）；其余越界一律
//! 钳位 + warn（无人值守场景下拒绝启动的代价远大于按保守值跑）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::cfg_http::HttpConfig;
use super::cfg_runtime::RuntimeConfig;
use super::cfg_storage::StorageConfig;
use super::cfg_ticker::TickerConfig;
use super::env_expand::expand_env_placeholders;
use super::error::ConfigError;

/// 运行时配置。
///
/// 字段按需补齐：每个段在出现首个真实消费者时加入，并在这里写明热度。
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// HTTP 服务监听参数（消费者：api 的 `bind` / `build_router`）。
    ///
    /// 不可热变更：socket 在启动时一次 bind，改这里必须重启
    /// （`ConfigStore::reload` 会把本段回滚为运行值并列入 `requires_restart`）。
    pub http: HttpConfig,
    /// Tokio runtime 线程预算（消费者：app 的 `Runtimes::build`）。
    ///
    /// 不可热变更：runtime 在 `block_on` 之前建好，改这里必须重启。
    pub runtime: RuntimeConfig,
    /// 持久化后端（消费者：storage 的 `init_storage`）。
    ///
    /// 不可热变更：连接池与迁移在启动时一次成型，改这里必须重启
    /// （`ConfigStore::reload` 会把本段回滚为运行值并列入 `requires_restart`）。
    pub storage: StorageConfig,
    /// 示例任务面 `ticker`（消费者：worker 的 `ticker` + app 的注册闸）。
    ///
    /// `enabled` 半热（启动闸，归 deferred），其余可热：任务经 `ConfigHandle`
    /// 每 tick 现读，改动下一拍生效。
    pub ticker: TickerConfig,
}

/// 内嵌默认配置模板（缺文件时落盘；`load_or_init` 使用）。
///
/// 模板取值与 `Default` 必须逐字段一致
/// （`embedded_template_parses_and_equals_default` 钉住这一点）。
pub fn default_config_template() -> &'static str {
    include_str!("default.toml")
}

/// 加载（不落盘版本：文件必须存在）。启动与热重载共用。
///
/// `root` 是安装根，配置里所有相对路径的基准（布局见 `util::paths`）。由调用方给出
/// 而不是在这里推导：管线不碰 `current_exe()` 才好测，且「基准是谁」在调用点就看得见。
pub fn load(path: &Path, root: &Path) -> Result<AppConfig, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let expanded = expand_env_placeholders(&raw)?;
    let deserializer =
        toml::de::Deserializer::parse(&expanded).map_err(|error| ConfigError::Parse {
            path_in_doc: "(toml 语法)".to_string(),
            message: error.message().to_string(),
        })?;

    let mut config: AppConfig =
        serde_path_to_error::deserialize(deserializer).map_err(|error| ConfigError::Parse {
            path_in_doc: error.path().to_string(),
            message: error.inner().message().to_string(),
        })?;

    config.resolve_paths(root);
    config.validate()?;
    config.sanitize();
    Ok(config)
}

/// 缺文件时先落盘内嵌模板再加载（`boot_strap` / `ConfigStore::load_or_init` 用）。
pub fn load_or_init(path: &Path, root: &Path) -> Result<AppConfig, ConfigError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    if !path.exists() {
        std::fs::write(path, default_config_template()).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        tracing::info!(path = %path.display(), "config file missing, default template written");
    }

    load(path, root)
}

impl AppConfig {
    /// 相对路径解析：逐段委托，各子配置自己知道哪些字段是路径。
    /// `[http]` / `[runtime]` / `[ticker]` 没有路径字段，三个钩子都不涉及它们。
    fn resolve_paths(&mut self, root: &Path) {
        self.storage.resolve_paths(root);
    }

    /// 硬错误校验：只拦「继续跑必然失败」的取值。
    fn validate(&self) -> Result<(), ConfigError> {
        self.storage.validate()?;
        // 跨段校验：每个面的 `runtime` 绑定必须指向已配置的附加 runtime。
        // 不静默回落到主 runtime：回落会把「配置写错」藏成「性能不达预期」，那是最难查的一类故障
        for (section, binding) in [
            ("http", self.http.runtime.as_deref()),
            ("ticker", self.ticker.runtime.as_deref()),
        ] {
            if !self.runtime.has_runtime(binding) {
                return Err(ConfigError::Invalid(format!(
                    "[{section}].runtime = {:?} 不在 [runtime.extra] 里；请先声明 [runtime.extra.{}] 段，或删掉这个绑定回到主 runtime",
                    binding.unwrap_or_default(),
                    binding.unwrap_or_default()
                )));
            }
        }
        Ok(())
    }

    /// 软越界钳位（附 warn）。
    fn sanitize(&mut self) {
        self.http.sanitize();
        self.runtime.sanitize();
        self.storage.sanitize();
        self.ticker.sanitize();
    }

    /// `Default` 经过同一条管线后的样子：只有 SQLite 缺省路径会被推导，其余不变。
    /// 测试用来和 `load` 的结果比对——「模板即默认值」的判据要在管线**之后**比。
    #[cfg(test)]
    pub(crate) fn default_resolved(root: &Path) -> Self {
        let mut cfg = Self::default();
        cfg.resolve_paths(root);
        cfg
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::util::{DATA_DIR, config_file_name, default_config_path};

    /// 临时目录前缀。项目名只放在**常量声明**里：一旦让它进到表达式中间，这一行的
    /// 宽度就随项目名长短在 rustfmt 的阈值上下翻转，模板的 fmt 门禁便只对某些名字成立
    const DIR_PREFIX: &str = "{{crate_name}}_cfg";

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        let d = std::env::temp_dir().join(format!("{DIR_PREFIX}_{tag}_{pid}_{nanos}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// (安装根, 配置文件路径)。这里把临时目录直接当安装根、配置文件平铺在它下面：
    /// 本模块测的是管线，缺省布局里 `config/` 那一层由 `util::paths` 自己钉
    fn tmp_config(tag: &str) -> (PathBuf, PathBuf) {
        let dir = tmp_dir(tag);
        let path = dir.join(config_file_name());
        (dir, path)
    }

    /// 内嵌模板必须能被本管线读通，且等价于 Default（模板即默认值的镜像；
    /// 模板改了默认值没跟上——或反过来——这条立刻红）
    #[test]
    fn embedded_template_parses_and_equals_default() {
        let (dir, path) = tmp_config("tpl");
        let cfg = load_or_init(&path, &dir).expect("模板必须可读");
        assert!(path.exists(), "缺文件应落盘模板");
        assert_eq!(
            cfg,
            AppConfig::default_resolved(&dir),
            "模板取值必须与 Default 一致"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 缺省布局端到端：库落在安装根下的 `data/` 里，而不是配置文件旁、
    /// 更不是配置目录里面。用户可见的那一层，钉住
    #[test]
    fn default_layout_puts_the_db_in_a_sibling_data_dir() {
        let root = tmp_dir("layout");
        let path = default_config_path(&root);
        let cfg = load_or_init(&path, &root).expect("缺省布局必须可起");

        let db = PathBuf::from(&cfg.storage.sqlite.path);
        assert_eq!(db.parent().unwrap(), root.join(DATA_DIR), "库要在 data/ 里");
        assert_eq!(
            db.parent().unwrap().parent().unwrap(),
            path.parent().unwrap().parent().unwrap(),
            "data/ 与 config/ 同级"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 缺父目录也能起：缺省布局的 `config/` 与 `--config` 指向尚不存在的目录都靠这一步
    #[test]
    fn load_or_init_creates_missing_parent_dir() {
        let dir = tmp_dir("mkdir");
        let path = default_config_path(&dir);
        load_or_init(&path, &dir).expect("父目录缺失应自动创建");
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空文件等价默认：零配置可用是本管线的硬约束
    #[test]
    fn empty_file_falls_back_to_defaults() {
        let (dir, path) = tmp_config("empty");
        std::fs::write(&path, "").unwrap();
        let cfg = load(&path, &dir).expect("空配置必须可读");
        assert_eq!(cfg, AppConfig::default_resolved(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 管线必须真的调到 resolve_paths，且基准是**安装根**而不是配置文件所在目录。
    ///
    /// 夹具刻意用缺省布局（配置在 `<root>/config/` 里）而不是 `tmp_config`：后者把
    /// 配置平铺在 root 下，两种基准算出来一模一样，那样的夹具在错误规则下也照样绿
    #[test]
    fn load_resolves_relative_sqlite_path_against_the_install_root() {
        let root = tmp_dir("resolve");
        let path = default_config_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[storage.sqlite]\npath = \"db/x.db\"\n").unwrap();

        let cfg = load(&path, &root).unwrap();
        assert_eq!(
            cfg.storage.sqlite.path,
            root.join("db/x.db").to_string_lossy()
        );
        assert_ne!(
            cfg.storage.sqlite.path,
            path.parent().unwrap().join("db/x.db").to_string_lossy(),
            "基准不是配置文件所在目录"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 跨段校验：面绑到未声明的附加 runtime 是硬错误，信息要指名段与名字
    #[test]
    fn load_rejects_binding_to_an_undeclared_runtime() {
        let (dir, path) = tmp_config("rtbind");
        std::fs::write(&path, "[ticker]\nruntime = \"typo\"\n").unwrap();
        let error = load(&path, &dir).expect_err("未声明的 runtime 绑定必须阻断加载");
        let msg = error.to_string();
        assert!(
            msg.contains("[ticker].runtime") && msg.contains("typo"),
            "{msg}"
        );

        std::fs::write(
            &path,
            "[runtime.extra.compute]\nworker_threads = 1\n[ticker]\nruntime = \"compute\"\n",
        )
        .unwrap();
        let cfg = load(&path, &dir).expect("声明过的绑定合法");
        assert_eq!(cfg.ticker.runtime.as_deref(), Some("compute"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 管线必须真的调到 validate：非法 sslmode 在加载期就报错
    #[test]
    fn load_rejects_invalid_storage_value() {
        let (dir, path) = tmp_config("invalid");
        std::fs::write(&path, "[storage.postgres]\nsslmode = \"verify-full\"\n").unwrap();
        assert!(matches!(load(&path, &dir), Err(ConfigError::Invalid(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 字符串值可由环境变量注入（占位符展开在反序列化之前，与段无关）。
    ///
    /// 变量名刻意取「不会有人设置」的一支：这里断言的是**缺省**分支
    #[test]
    fn load_expands_env_placeholder_in_http_value() {
        let (dir, path) = tmp_config("envph");
        std::fs::write(
            &path,
            "[http]\nhost = \"${env.{{env_prefix}}_NEVER_SET_HTTP_HOST:127.0.0.1}\"\n",
        )
        .unwrap();
        let cfg = load(&path, &dir).unwrap();
        assert_eq!(cfg.http.host, "127.0.0.1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非法取值在加载期报 Parse 错误并带字段路径（u16 溢出的端口不该活到 bind 才被发现）
    #[test]
    fn load_rejects_out_of_range_http_port() {
        let (dir, path) = tmp_config("badport");
        std::fs::write(&path, "[http]\nport = 70000\n").unwrap();
        let error = load(&path, &dir).expect_err("溢出端口必须阻断加载");
        match error {
            ConfigError::Parse { path_in_doc, .. } => {
                assert!(
                    path_in_doc.contains("port"),
                    "错误应定位到字段: {path_in_doc}"
                )
            }
            other => panic!("期望 Parse 错误，实际是 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 拼错的段名是错误，不是静默忽略：`deny_unknown_fields` 对顶层同样生效
    #[test]
    fn load_rejects_unknown_section() {
        let (dir, path) = tmp_config("unknown");
        std::fs::write(&path, "[tickr]\ninterval_ms = 5\n").unwrap();
        assert!(matches!(load(&path, &dir), Err(ConfigError::Parse { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 管线必须真的调到 sanitize：软越界钳位而不拒绝启动
    #[test]
    fn load_clamps_soft_out_of_range_values() {
        let (dir, path) = tmp_config("clamp");
        std::fs::write(&path, "[ticker]\ninterval_ms = 1\n").unwrap();
        let cfg = load(&path, &dir).expect("软越界不得阻断启动");
        assert_eq!(cfg.ticker.interval_ms, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
