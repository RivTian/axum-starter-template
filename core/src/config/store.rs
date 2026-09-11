//! 配置的运行时分发：唯一写端 [`ConfigStore`] 与廉价读端 [`ConfigHandle`]。
//!
//! 写端只在装配层；读端随处注入，可热字段每 tick 经 `current()` 现读。
//! 热重载把「改了它能不能不重启就生效」分成三档（见 [`ReloadReport`]），
//! 不可热段回滚为运行值——watch 里的配置永远等于「生效中」的配置。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::watch;

use super::app::{AppConfig, load_or_init};
use super::error::ConfigError;

/// 一次热重载的结果清单（SIGHUP 日志用；字段路径用 serde 主名，如 `ticker.interval_ms`）。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadReport {
    /// 本次重载后的配置代数（每次成功重载 +1；启动装载为 0）
    pub generation: u64,
    /// 已生效的可热变更（消费任务下一 tick 现读到新值）
    pub applied: Vec<String>,
    /// 半热变更：仅当启动时该顶层任务已拉起才生效；启动即关不补拉起
    pub deferred: Vec<String>,
    /// 不可热变更：已回滚为运行值，需重启才生效
    pub requires_restart: Vec<String>,
}

impl ReloadReport {
    /// 三张清单都空（配置内容没有实质变化）
    pub fn is_noop(&self) -> bool {
        self.applied.is_empty() && self.deferred.is_empty() && self.requires_restart.is_empty()
    }
}

/// 配置写端：仅 app 装配层持有。
pub struct ConfigStore {
    tx: watch::Sender<Arc<AppConfig>>,
    path: PathBuf,
    root: PathBuf,
    generation: AtomicU64,
}

impl ConfigStore {
    /// 启动装载：缺文件先落盘内嵌模板；失败即启动失败（fail-fast）。
    ///
    /// `root` 是安装根（配置里相对路径的基准）。存下来是为了让热重载沿用**同一个**
    /// 基准：重载时重新推一次会让「把二进制换个位置」变成路径静默漂移。
    pub fn load_or_init(path: PathBuf, root: PathBuf) -> Result<Self, ConfigError> {
        let cfg = load_or_init(&path, &root)?;
        let (tx, _rx) = watch::channel(Arc::new(cfg));
        Ok(Self {
            tx,
            path,
            root,
            generation: AtomicU64::new(0),
        })
    }

    /// 派生一个只读句柄（Clone 廉价，随处注入）。
    pub fn handle(&self) -> ConfigHandle {
        ConfigHandle {
            rx: self.tx.subscribe(),
        }
    }

    /// 当前配置（`Arc` 克隆，非全量拷贝）。
    pub fn current(&self) -> Arc<AppConfig> {
        self.tx.borrow().clone()
    }

    /// 当前配置代数。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// 配置文件路径（诊断 / 日志用）。
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 热重载：SIGHUP 的入口（将来的 HTTP 端点也走它）。
    ///
    /// 全量重跑加载管线 → 白名单分类 → 不可热字段回滚 → `watch::send`。
    /// 加载失败直接返回 `Err`，watch 内容不动：保留 last-good，绝不半套用。
    /// 事件广播（`ConfigReloaded { generation }`）由调用方在成功后自行发出
    /// ——config 模块不依赖 events，保持层内单向。
    pub fn reload(&self) -> Result<ReloadReport, ConfigError> {
        let old = self.current();
        let mut fresh = load_or_init(&self.path, &self.root)?;

        let mut report = ReloadReport::default();
        diff_values(
            &to_value(&old),
            &to_value(&fresh),
            &mut String::new(),
            &mut report,
        );

        // 不可热段回滚为运行值：watch 内容始终等于「生效中」的配置。
        // 不回滚的后果是：读端看到新端口 / 新线程数，实际 socket 仍 bind 在旧端口、
        // runtime 仍是旧形状，任何依据配置做判断的代码都会说谎。
        // 不可热的几段在此硬编码并排回滚，与 classify 的前缀白名单逐字对齐；
        // 新增不可热段时两处一起改
        if !report.requires_restart.is_empty() {
            fresh.http = old.http.clone();
            fresh.runtime = old.runtime.clone();
            fresh.storage = old.storage.clone();
            // 面的绑定字段单独回滚：所在段其余字段可热，不能整段回滚
            fresh.ticker.runtime = old.ticker.runtime.clone();
            tracing::warn!(
                fields = ?report.requires_restart,
                "config reload: non-hot fields changed, keeping running values until restart"
            );
        }

        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        report.generation = generation;
        // send_replace 而不是 send：后者在「当前没有任何接收端」时拒绝写入，
        // 而句柄是随用随派生的——没有活跃句柄的瞬间不该让重载静默失效
        self.tx.send_replace(Arc::new(fresh));
        Ok(report)
    }
}

/// 配置读端：廉价 Clone 的注入句柄。
#[derive(Clone)]
pub struct ConfigHandle {
    rx: watch::Receiver<Arc<AppConfig>>,
}

impl ConfigHandle {
    /// 当前配置（`Arc` 克隆）。
    pub fn current(&self) -> Arc<AppConfig> {
        self.rx.borrow().clone()
    }

    /// 等到下一代配置发布。写端已销毁（进程关停）时返回 `None`——
    /// 消费循环据此退出等待而不是永久挂起。
    pub async fn changed(&mut self) -> Option<Arc<AppConfig>> {
        self.rx.changed().await.ok()?;
        Some(self.rx.borrow_and_update().clone())
    }
}

// ── diff 与白名单 ─────────────────────────────────────────────────────────────

/// 序列化成 `toml::Value` 做结构化 diff：新增配置字段自动按「所属段」获得
/// 热度分类，不必每加一个字段就来这里登记一次。
fn to_value(cfg: &AppConfig) -> toml::Value {
    toml::Value::try_from(cfg).expect("AppConfig 可序列化（结构体全字段 Serialize）")
}

/// 递归比对两棵配置树，把变更的叶子路径按白名单分类进报告。
///
/// **两侧键集合不一定一致**，哪怕来自同一个 Rust 类型：`Option` 字段为 `None` 时
/// toml 根本不出这个键（`ticker.runtime` 从没绑到绑上，就是「新侧独有」），
/// `[runtime.extra.<name>]` 这类 map 段更是随配置增删键。所以取并集遍历——
/// 只遍历旧侧会把「新增的字段/子段」当成没变，于是不进任何桶：既不报
/// requires_restart 也不回滚，冷字段被静默热生效，正是热重载最不该有的失败方式
fn diff_values(old: &toml::Value, new: &toml::Value, path: &mut String, report: &mut ReloadReport) {
    match (old, new) {
        (toml::Value::Table(a), toml::Value::Table(b)) => {
            // BTreeSet：报告顺序按路径字典序，跨次重载可复现
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let len = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(k);
                match (a.get(k), b.get(k)) {
                    (Some(av), Some(bv)) => diff_values(av, bv, path, report),
                    // 只在一侧出现：整棵子树算这一个路径上的一处变更，
                    // 按该路径分类（`runtime.extra.compute` 整段落 requires_restart）
                    _ => classify(path, report),
                }
                path.truncate(len);
            }
        }
        _ if old != new => classify(path, report),
        _ => {}
    }
}

/// 不可热段的前缀白名单。新增不可热段在此登记，并同步到 `reload` 的回滚列表。
///
/// - `http.`：socket 在启动时一次 bind，`TimeoutLayer` 在 `build_router` 时一次成型；
/// - `runtime.`：runtime 在 `block_on` 之前建好；
/// - `storage.`：连接池与迁移在启动时一次成型；
/// - `<面>.runtime`（按后缀判）：绑定在 spawn 那一刻定死。
///
/// 不登记的话它们会落进默认的 applied 桶，热重载**假报「已生效」**，比明说
/// 「要重启」更糟：运维照报告以为改完了，实际什么都没变
const COLD_PREFIXES: &[&str] = &["http.", "runtime.", "storage."];

/// 白名单分类：按「改了它能不能不重启就生效」归档。
/// 未登记者默认归 applied（可热变更）。
fn classify(path: &str, report: &mut ReloadReport) {
    let bucket = if COLD_PREFIXES.iter().any(|p| path.starts_with(p)) || path.ends_with(".runtime")
    {
        &mut report.requires_restart
    } else if path == "enabled" || path.ends_with(".enabled") {
        // 开关类字段半热：新值已进 watch，但启动闸只在启动时读一次——启动即关
        // 的面不会因热重载补拉起，所以归 deferred 而不是 applied。
        // 必须用真实的后缀判断：`matches!(path, "**.enabled")` 是字面量相等比较
        // 而非 glob，永不命中
        &mut report.deferred
    } else {
        &mut report.applied
    };
    bucket.push(path.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录前缀。项目名只放在**常量声明**里：一旦让它进到表达式中间，这一行的
    /// 宽度就随项目名长短在 rustfmt 的阈值上下翻转，模板的 fmt 门禁便只对某些名字成立
    const DIR_PREFIX: &str = "{{crate_name}}_cfg_store";

    fn tmp_config(tag: &str, body: &str) -> (PathBuf, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("{DIR_PREFIX}_{tag}_{pid}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(crate::util::config_file_name());
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    /// 白名单分类的路径判据：`.enabled` 后缀（含裸 `enabled`）→ deferred；
    /// `http.` / `runtime.` 前缀 → requires_restart；其余 → applied。
    /// 特别钉住反例：形如 `xenabled` 的伪后缀、`httpx.` 的伪前缀不得命中
    #[test]
    fn classify_buckets_paths_by_real_affixes() {
        let mut report = ReloadReport::default();
        for path in [
            "enabled",
            "ticker.enabled",
            "http.host",
            "http.port",
            "runtime.worker_threads",
            "storage.sqlite.path",
            "ticker.runtime",
            "ticker.interval_ms",
            "ticker.xenabled",
            "httpx.port",
        ] {
            classify(path, &mut report);
        }

        assert_eq!(report.deferred, vec!["enabled", "ticker.enabled"]);
        assert_eq!(
            report.requires_restart,
            vec![
                "http.host",
                "http.port",
                "runtime.worker_threads",
                "storage.sqlite.path",
                "ticker.runtime"
            ]
        );
        assert_eq!(
            report.applied,
            vec!["ticker.interval_ms", "ticker.xenabled", "httpx.port"]
        );
    }

    #[test]
    fn reload_without_change_is_noop_but_bumps_generation() {
        let (dir, path) = tmp_config("noop", "");
        let store = ConfigStore::load_or_init(path, dir.clone()).unwrap();
        assert_eq!(store.generation(), 0);

        let report = store.reload().unwrap();
        assert!(report.is_noop());
        assert_eq!(report.generation, 1);
        assert_eq!(store.generation(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 可热字段：新值进 watch，读端下一次 current() 就能看到，且 changed() 被唤醒
    #[tokio::test]
    async fn reload_applies_hot_fields_and_wakes_handles() {
        let (dir, path) = tmp_config("hot", "[ticker]\ninterval_ms = 1000\n");
        let store = ConfigStore::load_or_init(path.clone(), dir.clone()).unwrap();
        let mut handle = store.handle();

        std::fs::write(&path, "[ticker]\ninterval_ms = 2000\n").unwrap();
        let report = store.reload().unwrap();

        assert_eq!(report.applied, vec!["ticker.interval_ms"]);
        assert_eq!(handle.current().ticker.interval_ms, 2000);
        let next = handle.changed().await.expect("写端仍在");
        assert_eq!(next.ticker.interval_ms, 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不可热段变更：进 requires_restart，且 watch 里的值必须仍是运行值
    #[test]
    fn reload_rolls_back_cold_sections() {
        let (dir, path) = tmp_config("cold", "[http]\nport = 8080\n");
        let store = ConfigStore::load_or_init(path.clone(), dir.clone()).unwrap();
        let handle = store.handle();

        std::fs::write(
            &path,
            "[http]\nport = 9090\n\n[runtime]\nworker_threads = 2\n\n[storage.sqlite]\nread_pool_size = 9\n",
        )
        .unwrap();
        let report = store.reload().unwrap();

        assert!(report.applied.is_empty());
        assert_eq!(
            report.requires_restart,
            vec![
                "http.port",
                "runtime.worker_threads",
                "storage.sqlite.read_pool_size"
            ]
        );
        assert_eq!(handle.current().http.port, 8080, "冷段必须回滚为运行值");
        assert_eq!(handle.current().runtime.worker_threads, 0);
        assert_eq!(handle.current().storage.sqlite.read_pool_size, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 只在一侧出现的键也算变更：`Option` 字段 None→Some / Some→None，
    /// 以及 `[runtime.extra.<name>]` 这种 map 段的增删。回归用：只遍历旧侧时
    /// 这些变更会被静默吞掉
    #[test]
    fn keys_present_on_only_one_side_are_still_reported() {
        let (dir, path) = tmp_config("onesided", "[ticker]\ninterval_ms = 1000\n");
        let store = ConfigStore::load_or_init(path.clone(), dir.clone()).unwrap();

        // None → Some：旧侧没有 ticker.runtime 这个键
        std::fs::write(
            &path,
            "[runtime.extra.compute]\nworker_threads = 1\n[ticker]\ninterval_ms = 1000\nruntime = \"compute\"\n",
        )
        .unwrap();
        let report = store.reload().unwrap();
        assert!(report.applied.is_empty(), "{report:?}");
        assert_eq!(
            report.requires_restart,
            vec!["runtime.extra.compute", "ticker.runtime"],
            "新增的 map 子段与新增的绑定字段都要报出来"
        );

        // Some → None：这次是新侧没有这个键。回滚过一次，运行值里 ticker.runtime
        // 仍是 None，所以改回不绑等于无变更
        std::fs::write(&path, "[ticker]\ninterval_ms = 1000\n").unwrap();
        let report = store.reload().unwrap();
        assert!(
            report.is_noop(),
            "回滚后运行值本就没绑，改回不绑无变更: {report:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 面的绑定字段冷：进 requires_restart 且回滚，所在段其余可热字段照常生效
    #[test]
    fn reload_rolls_back_only_the_binding_field_of_a_hot_section() {
        let (dir, path) = tmp_config("bind", "[ticker]\ninterval_ms = 1000\n");
        let store = ConfigStore::load_or_init(path.clone(), dir.clone()).unwrap();

        std::fs::write(
            &path,
            "[runtime.extra.compute]\nworker_threads = 1\n[ticker]\ninterval_ms = 2000\nruntime = \"compute\"\n",
        )
        .unwrap();
        let report = store.reload().unwrap();

        assert_eq!(report.applied, vec!["ticker.interval_ms"]);
        assert!(
            report
                .requires_restart
                .contains(&"ticker.runtime".to_string())
        );
        let current = store.current();
        assert_eq!(current.ticker.interval_ms, 2000, "可热字段生效");
        assert_eq!(current.ticker.runtime, None, "绑定字段回滚");
        assert!(current.runtime.extra.is_empty(), "runtime 段整段回滚");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 启动闸半热：归 deferred，不归 applied
    #[test]
    fn reload_marks_enable_gates_as_deferred() {
        let (dir, path) = tmp_config("gate", "[ticker]\nenabled = true\n");
        let store = ConfigStore::load_or_init(path.clone(), dir.clone()).unwrap();

        std::fs::write(&path, "[ticker]\nenabled = false\n").unwrap();
        let report = store.reload().unwrap();

        assert_eq!(report.deferred, vec!["ticker.enabled"]);
        assert!(report.applied.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重载失败保留 last-good：watch 不动、代数不动
    #[test]
    fn failed_reload_keeps_last_good() {
        let (dir, path) = tmp_config("bad", "[ticker]\ninterval_ms = 1000\n");
        let store = ConfigStore::load_or_init(path.clone(), dir.clone()).unwrap();

        std::fs::write(&path, "[ticker]\ninterval_ms = \"not a number\"\n").unwrap();
        assert!(store.reload().is_err());

        assert_eq!(store.current().ticker.interval_ms, 1000);
        assert_eq!(store.generation(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
