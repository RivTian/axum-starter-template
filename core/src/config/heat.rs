//! 热度判别：**漏登记一个字段，编译就红。**
//!
//! # 为什么不用前缀白名单
//!
//! 一种看起来更省事的写法是"冷字段名以这些前缀开头"。它的失效方式是静默的：新增一个冷字段而忘了让它
//! 匹配前缀，重载会把它判成热的，报告说"已生效"，而进程里那个值一动没动。真值与生效中
//! 的配置就此分叉，而**没有任何东西会变红**。
//!
//! 这里换成穷尽解构。加一个字段，要连过三道编译错误才能通过：
//!
//! 1. [`classify`] 里 `let Config { .. }` 不穷尽 → 必须回答它属于哪一段；
//! 2. 若归入冷段，[`ColdView`] 的构造不完整 → 必须把它加进冷视图；
//! 3. [`ColdView::changed_fields`] 里的解构不穷尽 → 必须给它一条报告路径。
//!
//! 三步走完，这个字段就同时有了热度、有了比较、有了报告路径。**没有一步可以跳过**，
//! 因为它们都是编译错误而不是约定。
//!
//! # 两套判据，一强一弱
//!
//! - "有没有冷段变更"用 [`ColdView`] 的**类型全等**。它不关心是哪个字段变的，因此不会
//!   因为某条比较写漏而放过变更。这是**决定是否拒绝重载**的那一条。
//! - "是哪些冷字段变了"用逐字段比较。它只用于**报告**。
//!
//! 强判据管决策、弱判据管展示。就算 `changed_fields` 有一天漏了一条，结果也只是报告不够
//! 详细，而不是把一次冷变更放行——[`cold_change_is_never_silently_accepted`] 守这一点。

use std::net::SocketAddr;

use super::publish::Accepted;
use super::types::{
    Config, HttpConfig, RuntimeConfig, StorageConfig, TelemetryConfig, WorkerConfig,
};
use crate::shutdown::Budgets;

/// 一次新旧配置比较的结果。
///
/// 报告与回滚都从这一份推导，**不存在第二份清单**（把清单写成两份的话，待重启清单
/// 是人工维护的，与实际判定各说各话）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeatDiff {
    cold_fields: Vec<&'static str>,
    cold_differs: bool,
    semi_fields: Vec<&'static str>,
    hot_fields: Vec<&'static str>,
}

impl HeatDiff {
    /// 是否含冷段变更。**权威判据**，来自类型全等而不是逐字段比较。
    #[must_use]
    pub const fn has_cold_change(&self) -> bool {
        self.cold_differs
    }

    /// 变更的冷字段路径。仅供报告，可能比 [`Self::has_cold_change`] 少。
    #[must_use]
    pub fn cold_fields(&self) -> &[&'static str] {
        &self.cold_fields
    }

    /// 变更的半热字段路径——即"下次面重启生效"的那一批。
    #[must_use]
    pub fn semi_fields(&self) -> &[&'static str] {
        &self.semi_fields
    }

    /// 变更的热字段路径——即"已经生效"的那一批。
    #[must_use]
    pub fn hot_fields(&self) -> &[&'static str] {
        &self.hot_fields
    }

    /// 新旧两份完全一致。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.cold_differs && self.semi_fields.is_empty() && self.hot_fields.is_empty()
    }
}

/// 一次重载请求的处理结论。
///
/// 把"是否接受"做成类型，而不是让装配层自己看着 [`HeatDiff`] 判断——判断只写一次，
/// 就不会出现两处判断规则不一致。
#[derive(Debug)]
pub enum ReloadDecision {
    /// 可以换值。`diff` 为空时表示新旧一致，装配层可以连 `send_replace` 都省掉。
    Accept {
        /// 要发布的新配置。
        ///
        /// 类型是 [`Accepted`] 而不是 `Config`：它是 [`ConfigPublisher::publish`] 唯一
        /// 接受的输入，于是"发布一份含冷段变更的配置"在类型上就无从发生。
        ///
        /// [`ConfigPublisher::publish`]: super::ConfigPublisher::publish
        config: Accepted,
        /// 变更明细。
        diff: HeatDiff,
    },
    /// 含冷段变更，**整批拒绝**，保留 last-good。
    ///
    /// 不做"热的先生效、冷的等重启"：那会让真值与生效中的配置分叉，而这个模块的全部意义
    /// 就是这两者恒等。
    RejectCold {
        /// 变更明细，`cold_fields` 即待重启清单。
        diff: HeatDiff,
    },
}

/// 判断一次重载能否被接受。
///
/// 这是重载事务的**全部策略**，纯函数。装配层只负责把结论落地（`send_replace` 或记一条
/// 拒绝日志），不再做任何判断。
#[must_use]
pub fn evaluate_reload(current: &Config, next: Config) -> ReloadDecision {
    let diff = classify(current, &next);
    if diff.has_cold_change() {
        ReloadDecision::RejectCold { diff }
    } else {
        ReloadDecision::Accept {
            config: Accepted::new(next),
            diff,
        }
    }
}

/// 冷字段的只读视图。
///
/// 存在的唯一理由是那个 `PartialEq`：它让"有没有冷段变更"成为一次类型级比较。
///
/// `storage` / `runtime` / `shutdown` / `telemetry` 整段都是冷的，因此整段引用；
/// `http` 与 `worker` 是拆开的（它们各有字段分属别的档）。
///
/// 两个面的 `runtime` 键在这里：把一个已经在跑的面挪到另一个 runtime 上，等于把它停掉
/// 再在别处起一遍。那不是"换个值"，是重启。
#[derive(Debug, PartialEq, Eq)]
struct ColdView<'a> {
    http_bind_addr: &'a SocketAddr,
    http_runtime: Option<&'a str>,
    worker_runtime: Option<&'a str>,
    storage: &'a StorageConfig,
    runtime: &'a RuntimeConfig,
    shutdown: &'a Budgets,
    telemetry: &'a TelemetryConfig,
}

impl<'a> ColdView<'a> {
    fn of(config: &'a Config) -> Self {
        // 无 `..`：`Config` 新增字段时这里也编译不过，逼人回答"它是不是冷的"。
        let Config {
            http:
                HttpConfig {
                    bind_addr,
                    handler_timeout: _,
                    body_limit_bytes: _,
                    runtime: http_runtime,
                },
            storage,
            worker:
                WorkerConfig {
                    enabled: _,
                    tick_interval: _,
                    runtime: worker_runtime,
                },
            runtime,
            shutdown,
            telemetry,
        } = config;

        Self {
            http_bind_addr: bind_addr,
            http_runtime: http_runtime.as_deref(),
            worker_runtime: worker_runtime.as_deref(),
            storage,
            runtime,
            shutdown,
            telemetry,
        }
    }

    /// 逐字段比较，产出报告路径。
    ///
    /// 只用于报告——决策用 `self != other`。
    fn changed_fields(&self, other: &Self) -> Vec<&'static str> {
        let Self {
            http_bind_addr,
            http_runtime,
            worker_runtime,
            storage,
            runtime,
            shutdown,
            telemetry,
        } = self;
        let Self {
            http_bind_addr: other_bind_addr,
            http_runtime: other_http_runtime,
            worker_runtime: other_worker_runtime,
            storage: other_storage,
            runtime: other_runtime,
            shutdown: other_shutdown,
            telemetry: other_telemetry,
        } = other;

        let mut out = Vec::new();
        push_if(
            &mut out,
            http_bind_addr != other_bind_addr,
            "http.bind_addr",
        );
        push_if(&mut out, http_runtime != other_http_runtime, "http.runtime");
        push_if(
            &mut out,
            worker_runtime != other_worker_runtime,
            "worker.runtime",
        );
        push_storage(&mut out, storage, other_storage);
        push_runtime(&mut out, runtime, other_runtime);
        push_shutdown(&mut out, shutdown, other_shutdown);
        push_telemetry(&mut out, telemetry, other_telemetry);
        out
    }
}

fn push_if(out: &mut Vec<&'static str>, changed: bool, path: &'static str) {
    if changed {
        out.push(path);
    }
}

fn push_storage(out: &mut Vec<&'static str>, a: &StorageConfig, b: &StorageConfig) {
    let StorageConfig {
        path,
        max_readers,
        busy_timeout,
        acquire_timeout,
    } = a;
    push_if(out, path != &b.path, "storage.path");
    push_if(out, max_readers != &b.max_readers, "storage.max_readers");
    push_if(out, busy_timeout != &b.busy_timeout, "storage.busy_timeout");
    push_if(
        out,
        acquire_timeout != &b.acquire_timeout,
        "storage.acquire_timeout",
    );
}

fn push_runtime(out: &mut Vec<&'static str>, a: &RuntimeConfig, b: &RuntimeConfig) {
    let RuntimeConfig {
        worker_threads,
        max_blocking_threads,
        extra,
    } = a;
    push_if(
        out,
        worker_threads != &b.worker_threads,
        "runtime.worker_threads",
    );
    push_if(
        out,
        max_blocking_threads != &b.max_blocking_threads,
        "runtime.max_blocking_threads",
    );
    // 整张表一条路径：报告要的是"重启才能生效"，不是"哪个附加 runtime 的哪个旋钮变了"。
    // 逐个列出来的话，路径里会带上用户起的名字，而这里的返回类型是 `&'static str`
    // ——那正是它该是 `&'static str` 的理由（冷段清单会进日志，而日志里不放取值）。
    push_if(out, extra != &b.extra, "runtime.extra");
}

fn push_shutdown(out: &mut Vec<&'static str>, a: &Budgets, b: &Budgets) {
    let Budgets {
        total_grace,
        harvest,
        reap,
        storage_close,
        runtime_shutdown,
        escalate,
    } = a;
    push_if(out, total_grace != &b.total_grace, "shutdown.total_grace");
    push_if(out, harvest != &b.harvest, "shutdown.harvest");
    push_if(out, reap != &b.reap, "shutdown.reap");
    push_if(
        out,
        storage_close != &b.storage_close,
        "shutdown.storage_close",
    );
    push_if(
        out,
        runtime_shutdown != &b.runtime_shutdown,
        "shutdown.runtime_shutdown",
    );
    push_if(out, escalate != &b.escalate, "shutdown.escalate");
}

fn push_telemetry(out: &mut Vec<&'static str>, a: &TelemetryConfig, b: &TelemetryConfig) {
    let TelemetryConfig { filter, format } = a;
    push_if(out, filter != &b.filter, "telemetry.filter");
    push_if(out, format != &b.format, "telemetry.format");
}

/// 比较新旧配置，给出三档变更清单。
pub(crate) fn classify(cur: &Config, new: &Config) -> HeatDiff {
    // 无 `..`：这是三道编译期关卡里的第一道。
    let Config {
        http:
            HttpConfig {
                bind_addr: _,
                handler_timeout,
                body_limit_bytes,
                runtime: _,
            },
        storage: _,
        worker:
            WorkerConfig {
                enabled,
                tick_interval,
                runtime: _,
            },
        runtime: _,
        shutdown: _,
        telemetry: _,
    } = cur;
    let Config {
        http:
            HttpConfig {
                bind_addr: _,
                handler_timeout: new_handler_timeout,
                body_limit_bytes: new_body_limit_bytes,
                runtime: _,
            },
        storage: _,
        worker:
            WorkerConfig {
                enabled: new_enabled,
                tick_interval: new_tick_interval,
                runtime: _,
            },
        runtime: _,
        shutdown: _,
        telemetry: _,
    } = new;

    let cold_view = ColdView::of(cur);
    let new_cold_view = ColdView::of(new);

    let mut semi_fields = Vec::new();
    push_if(
        &mut semi_fields,
        body_limit_bytes != new_body_limit_bytes,
        "http.body_limit_bytes",
    );
    push_if(&mut semi_fields, enabled != new_enabled, "worker.enabled");

    let mut hot_fields = Vec::new();
    push_if(
        &mut hot_fields,
        handler_timeout != new_handler_timeout,
        "http.handler_timeout",
    );
    push_if(
        &mut hot_fields,
        tick_interval != new_tick_interval,
        "worker.tick_interval",
    );

    HeatDiff {
        cold_fields: cold_view.changed_fields(&new_cold_view),
        cold_differs: cold_view != new_cold_view,
        semi_fields,
        hot_fields,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::path::PathBuf;
    use std::time::Duration;

    use super::super::types::{LogFormat, RuntimeThreads};
    use super::*;

    /// 「字段路径 + 把它改掉的那一步」。
    ///
    /// 起个名字是为了让下面三张表读起来只剩内容。裸写 `Vec<(&'static str, fn(&mut Config))>`
    /// 也能编过，但 `clippy::type_complexity` 会判红——而它判得对：这个类型在一行里同时
    /// 出现三次时确实认不出来。
    type Mutator = (&'static str, fn(&mut Config));

    /// 每个冷字段一个 mutator。
    ///
    /// 这份清单**不可能悄悄过期**：给 `Config` 加一个冷字段，`ColdView::of` 和
    /// `ColdView::changed_fields` 会先编译失败，加完之后 `every_cold_field_is_reported`
    /// 会因为下面的 `expected` 里没有它而红——见 `cold_field_list_is_complete`。
    fn cold_mutators() -> Vec<Mutator> {
        vec![
            ("http.bind_addr", |c| {
                c.http.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9999);
            }),
            // 把一个在跑的面挪到另一个 runtime 上，等于把它停掉再在别处起一遍。那是重启，
            // 不是换值——所以这两条在冷表里，而不是在半热表里。
            ("http.runtime", |c| c.http.runtime = Some("io".to_owned())),
            ("worker.runtime", |c| {
                c.worker.runtime = Some("io".to_owned());
            }),
            ("storage.path", |c| {
                c.storage.path = PathBuf::from("data/other.sqlite3");
            }),
            ("storage.max_readers", |c| c.storage.max_readers += 1),
            ("storage.busy_timeout", |c| {
                c.storage.busy_timeout += Duration::from_secs(1);
            }),
            ("storage.acquire_timeout", |c| {
                c.storage.acquire_timeout += Duration::from_secs(1);
            }),
            ("runtime.worker_threads", |c| {
                c.runtime.worker_threads = Some(3);
            }),
            ("runtime.max_blocking_threads", |c| {
                c.runtime.max_blocking_threads = Some(7);
            }),
            // 整张表一条路径：改动明细会进日志，而附加 runtime 的名字是用户起的，
            // 逐条列出来就要往 `&'static str` 里塞运行期字符串。
            ("runtime.extra", |c| {
                c.runtime
                    .extra
                    .insert("io".to_owned(), RuntimeThreads::default());
            }),
            ("shutdown.total_grace", |c| {
                c.shutdown.total_grace += Duration::from_secs(1);
            }),
            ("shutdown.harvest", |c| {
                c.shutdown.harvest += Duration::from_secs(1);
            }),
            ("shutdown.reap", |c| {
                c.shutdown.reap += Duration::from_secs(1)
            }),
            ("shutdown.storage_close", |c| {
                c.shutdown.storage_close += Duration::from_secs(1);
            }),
            ("shutdown.runtime_shutdown", |c| {
                c.shutdown.runtime_shutdown += Duration::from_secs(1);
            }),
            ("shutdown.escalate", |c| {
                c.shutdown.escalate += Duration::from_secs(1);
            }),
            ("telemetry.filter", |c| {
                c.telemetry.filter = String::from("debug");
            }),
            ("telemetry.format", |c| c.telemetry.format = LogFormat::Json),
        ]
    }

    fn semi_mutators() -> Vec<Mutator> {
        vec![
            ("http.body_limit_bytes", |c| c.http.body_limit_bytes += 1),
            ("worker.enabled", |c| c.worker.enabled = !c.worker.enabled),
        ]
    }

    fn hot_mutators() -> Vec<Mutator> {
        vec![
            ("http.handler_timeout", |c| {
                c.http.handler_timeout += Duration::from_secs(1);
            }),
            ("worker.tick_interval", |c| {
                c.worker.tick_interval += Duration::from_secs(1);
            }),
        ]
    }

    fn mutated(f: fn(&mut Config)) -> Config {
        let mut c = Config::default();
        f(&mut c);
        c
    }

    #[test]
    fn identical_configs_produce_an_empty_diff() {
        let diff = classify(&Config::default(), &Config::default());
        assert!(diff.is_empty());
        assert!(!diff.has_cold_change());
    }

    #[test]
    fn every_cold_field_is_reported_on_its_own_path() {
        let base = Config::default();
        for (i, (path, mutate)) in cold_mutators().into_iter().enumerate() {
            let diff = classify(&base, &mutated(mutate));
            assert!(diff.has_cold_change(), "TC{i} ({path}) 必须被判成冷段变更");
            assert_eq!(diff.cold_fields(), [path], "TC{i} ({path}) 的报告路径不符");
            assert!(diff.semi_fields().is_empty(), "TC{i} ({path}) 不该串到半热");
            assert!(diff.hot_fields().is_empty(), "TC{i} ({path}) 不该串到热");
        }
    }

    #[test]
    fn cold_field_list_is_complete() {
        // 把"清单有没有漏"变成一条会红的断言。
        //
        // 做法是拿 mutator 清单里的路径，去跟"把每个冷字段都改一遍"之后的报告对账。
        // 漏掉一个冷字段的唯一方式是既不给它 mutator、也不给它报告路径——而后者会先
        // 在 `ColdView::changed_fields` 的穷尽解构上编译失败。
        let base = Config::default();
        let mut all = Config::default();
        let expected: Vec<&'static str> = cold_mutators()
            .into_iter()
            .map(|(path, mutate)| {
                mutate(&mut all);
                path
            })
            .collect();

        let diff = classify(&base, &all);
        assert_eq!(
            diff.cold_fields(),
            expected.as_slice(),
            "全部冷字段同时变更时，报告必须逐条列出且顺序稳定"
        );
    }

    #[test]
    fn semi_and_hot_fields_land_in_their_own_buckets() {
        let base = Config::default();
        for (i, (path, mutate)) in semi_mutators().into_iter().enumerate() {
            let diff = classify(&base, &mutated(mutate));
            assert!(!diff.has_cold_change(), "TC{i} ({path}) 不该被判成冷");
            assert_eq!(diff.semi_fields(), [path], "TC{i} ({path}) 应落在半热");
            assert!(diff.hot_fields().is_empty());
        }
        for (i, (path, mutate)) in hot_mutators().into_iter().enumerate() {
            let diff = classify(&base, &mutated(mutate));
            assert!(!diff.has_cold_change(), "TC{i} ({path}) 不该被判成冷");
            assert_eq!(diff.hot_fields(), [path], "TC{i} ({path}) 应落在热");
            assert!(diff.semi_fields().is_empty());
        }
    }

    #[test]
    fn cold_change_is_never_silently_accepted() {
        // 强判据（类型全等）与弱判据（逐字段）分工的回归：只要冷视图不等，就必须拒绝，
        // 哪怕逐字段比较一条都没报出来。
        for (i, (path, mutate)) in cold_mutators().into_iter().enumerate() {
            let next = mutated(mutate);
            match evaluate_reload(&Config::default(), next) {
                ReloadDecision::RejectCold { diff } => {
                    assert!(
                        !diff.cold_fields().is_empty(),
                        "TC{i} ({path}) 拒绝时必须给出待重启清单"
                    );
                }
                ReloadDecision::Accept { .. } => {
                    panic!("TC{i} ({path}) 含冷段变更却被接受了")
                }
            }
        }
    }

    #[test]
    fn a_hot_only_change_is_accepted_whole() {
        let mut next = Config::default();
        next.http.handler_timeout += Duration::from_secs(1);
        next.worker.tick_interval += Duration::from_secs(1);

        match evaluate_reload(&Config::default(), next.clone()) {
            ReloadDecision::Accept { config, diff } => {
                assert_eq!(
                    config.peek(),
                    &next,
                    "接受时发布的必须是完整的新配置，不是逐字段合并"
                );
                assert_eq!(diff.hot_fields().len(), 2);
                assert!(diff.cold_fields().is_empty());
            }
            ReloadDecision::RejectCold { diff } => {
                panic!("纯热变更被拒绝了，冷清单是 {:?}", diff.cold_fields())
            }
        }
    }

    #[test]
    fn a_mixed_change_is_rejected_whole_not_partially_applied() {
        // 绝不半套用：冷 + 热混在一起时，热的那一半也不生效。
        let mut next = Config::default();
        next.http.handler_timeout += Duration::from_secs(1); // 热
        next.storage.max_readers += 1; // 冷

        match evaluate_reload(&Config::default(), next) {
            ReloadDecision::RejectCold { diff } => {
                assert_eq!(diff.cold_fields(), ["storage.max_readers"]);
                assert_eq!(
                    diff.hot_fields(),
                    ["http.handler_timeout"],
                    "被拒绝时热字段仍要出现在报告里——运维需要知道这一次改动整体没生效"
                );
            }
            ReloadDecision::Accept { .. } => panic!("混合变更必须整批拒绝"),
        }
    }
}
