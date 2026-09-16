//! 日志捕获。
//!
//! # 为什么这不能用 `tracing::subscriber::with_default` 写
//!
//! `with_default` 装的是**线程局部**的默认 subscriber，看起来正是测试想要的东西：一条用例
//! 一个 subscriber，互不干扰。它在并行测试下不成立，原因在 `tracing` 的一个性能设计：
//!
//! 每个 `info!` 调用点（callsite）第一次被执行时，`tracing-core` 会问一次"有人对它感兴趣
//! 吗"，把答案缓存下来，之后直接用缓存。这个缓存是**进程级**的，不是线程级的。于是在
//! `cargo test` 的默认并行下：线程 A 装了 subscriber 并触发了某个 callsite，缓存记下
//! "有人要"；线程 B 此刻没装 subscriber，同一个 callsite 却因为缓存而仍被判为"有人要"——
//! 反过来也一样，先被判成"没人要"的 callsite，在 A 装上 subscriber 之后依然是"没人要"，
//! A 的断言于是对着一个空列表跑。
//!
//! 症状是**偶发**的：单跑绿、全量跑红，或者反过来。这类失败最消耗人，因为第一反应总是
//! 去查被测代码。
//!
//! 这里换一条路：**进程里只装一个全局 subscriber**（装的那一刻 `tracing` 会重建兴趣缓存，
//! 之后再没有第二次变更），它对所有 callsite 一律"感兴趣"，于是缓存永远是"要"。真正的
//! 开关下沉到 `on_event` 里的一个 sink 槽位——没有 sink 时直接返回，连格式化都不做。
//!
//! # 捕获期是互斥的
//!
//! sink 是**全局**的，不是线程局部的。线程局部看起来更省事，但本模板要断言的那些事件
//! （`task_exit` / `storage_close_result`）由 tokio 的工作线程发出，线程局部 sink 恰好
//! 看不见它们——那样写出来的顺序断言会永远通过。
//!
//! 代价是同时只能有一个 [`LogCapture`] 存活：[`LogCapture::start`] 会等前一个 drop。
//! 于是**做日志断言的用例之间是串行的**，其余用例不受影响。
//!
//! 还有一个后果要记住：捕获期间**别的并行用例产生的事件也会落进同一个 sink**。所以断言
//! 一律按事件名走（[`LogCapture::find`] / [`LogCapture::position`]），不要断言"总共几条"。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Condvar, LockResult, Mutex, OnceLock, PoisonError, RwLock};

use tracing::field::{Field, Visit};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

// ── 全局状态 ────────────────────────────────────────────────────────────────
//
// 这是整个 workspace 里唯一一处全局可变状态，而它只存在于测试二进制中：`testkit` 是
// dev-only 成员，没有任何 crate 在 `[dependencies]` 里写它，它进不了发布产物。
//「没有全局可变状态」那条纪律管的是运行期的库层与 `app`；一个只在 `cargo test` 里被链接进来
// 的夹具不在那条纪律的射程内——而它要替代的东西（每个 crate 各抄一份 callsite 规避）
// 才是真正会出事的那个。

/// 全局 subscriber 只装一次，结果存下来供后续调用回放。
static INSTALL: OnceLock<Result<(), InstallError>> = OnceLock::new();

/// 当前活跃的 sink。`None` = 没人在捕获，`on_event` 直接返回。
static SINK: RwLock<Option<Arc<Mutex<Vec<CapturedEvent>>>>> = RwLock::new(None);

/// 捕获期互斥。
///
/// 刻意**不是** `Mutex<()>` + 长期持有 `MutexGuard`：那样 [`LogCapture`] 会变成 `!Send`，
/// 而且在 `async` 用例里跨 `.await` 持有它会被 `clippy::await_holding_lock`（我们设成
/// `deny`）直接判红。这里用「布尔 + 条件变量」，锁只在进出的那一瞬间持有。
static BUSY: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// 中毒的锁照常用。
///
/// 一条用例 panic 会把它当时持有的锁标记为中毒。这里守的东西只是一个事件列表，没有"可能
/// 处于半更新状态"的不变量可言；为此让**其余**用例全部连坐地 panic，只会把"一条失败"变成
/// "一片失败"，真正出问题的那条反而更难找。
fn unpoison<T>(result: LockResult<T>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

// ── 安装 ────────────────────────────────────────────────────────────────────

/// 全局 subscriber 已被别人占用。
///
/// 进程里只有一个全局 subscriber 槽位。本模板的纪律是除 `app` 与 `testkit` 外
/// 无人装 subscriber，所以这个错误在模板自身的测试里不可达；它存在是为了**让不可达变成
/// 可检测的**——真出现了，说明有一层越过了这条纪律，而那件事值得一条明确的错误而不是静默失效。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallError;

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a global tracing subscriber was already installed by someone else")
    }
}

impl std::error::Error for InstallError {}

/// 装上捕获用的全局 subscriber。**幂等**：重复调用返回第一次的结果。
///
/// [`LogCapture::start`] 会自己调用它，一般不需要显式调。显式调用的场景是想在用例里
/// 单独断言"这个测试二进制里没有别人抢 subscriber"。
///
/// # Errors
///
/// 全局 subscriber 已被本模块以外的代码占用时返回 [`InstallError`]。
pub fn install() -> Result<(), InstallError> {
    *INSTALL.get_or_init(|| {
        let subscriber = tracing_subscriber::registry().with(CaptureLayer);
        tracing::subscriber::set_global_default(subscriber).map_err(|_| InstallError)
    })
}

// ── 捕获到的事件 ────────────────────────────────────────────────────────────

/// 一条捕获到的事件。
///
/// 字段是**取值的字符串化**，不是原始类型。夹具要比的是"报告里写了什么"，而报告本身就是
/// 文本；保留原始类型会让断言被迫跟着字段类型走，改一次 `u32 → u64` 就要改一片用例。
#[derive(Clone, Debug)]
pub struct CapturedEvent {
    level: Level,
    target: String,
    name: &'static str,
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// 事件级别。
    #[must_use]
    pub const fn level(&self) -> Level {
        self.level
    }

    /// 事件 target（默认是发出事件的模块路径）。
    ///
    /// **不建议用它做断言**：模块路径以真实 crate 名开头，而真实 crate 名里有项目名。
    /// 写进用例就等于把项目名写进了 `.rs`，而钉住 lib 目标名的整条防线就是不让项目名进 Rust 源。
    /// 要定位事件请用 [`Self::name`]。
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// 事件名。
    ///
    /// 来自 `info!(name: "shutdown_started", …)`。没有显式写 `name:` 时，`tracing` 给的是
    /// `event <文件>:<行>` 这种自动名字——**那种事件不适合做断言**，因为它的名字会随行号漂。
    /// 需要被断言的事件都应当显式起名。
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// `message` 字段（`info!(…, "这句话")` 里的那句）。
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// 取一个结构化字段的字符串形式。
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// 全部结构化字段（不含 `message`）。
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }
}

/// 把事件的字段抄进 `BTreeMap`。
#[derive(Default)]
struct Collect {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl Collect {
    fn put(&mut self, key: &str, value: String) {
        if key == "message" {
            self.message = Some(value);
        } else {
            self.fields.insert(key.to_owned(), value);
        }
    }
}

impl Visit for Collect {
    /// `&str` 走这一条，拿到的是不带引号的原文。
    ///
    /// 不覆盖它的话字符串字段会经 `record_debug` 变成 `"value"`（带引号），断言里就得跟着
    /// 写引号——一个纯粹由实现细节造成的、每个用例都要记住的怪癖。
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field.name(), value.to_owned());
    }

    /// 其余类型（整数、布尔、`Debug`）统一走这里。`Visit` 的默认实现会把它们转发过来。
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.put(field.name(), format!("{value:?}"));
    }
}

// ── Layer ───────────────────────────────────────────────────────────────────

/// 对所有 callsite 一律"感兴趣"的捕获层。
///
/// 三个方法都必须覆盖，少一个就退回到"按级别过滤"，而级别过滤的结论同样会进那个
/// **进程级**缓存——本模块要绕开的就是它。
struct CaptureLayer;

impl<S> Layer<S> for CaptureLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(&self, _metadata: &Metadata<'_>) -> Interest {
        // `always` = 缓存成"要"，之后不再问。这正是我们想钉死的那一档：
        // 缓存里存的是一个**常量**，于是它是不是进程级的就无所谓了。
        Interest::always()
    }

    fn enabled(&self, _metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        // 不给这个提示的话，`tracing` 会按各层的提示求一个全局上界，`TRACE` 级事件可能
        // 在到达 `on_event` 之前就被丢掉。
        Some(tracing::level_filters::LevelFilter::TRACE)
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // 先把 Arc 取出来再放读锁：格式化事件可能不便宜，不该压着锁做。
        let sink = unpoison(SINK.read()).clone();
        let Some(sink) = sink else {
            // 没人在捕获——这是绝大多数事件走的路径，代价是一次读锁 + 一次判空。
            return;
        };

        let metadata = event.metadata();
        let mut collect = Collect::default();
        event.record(&mut collect);

        unpoison(sink.lock()).push(CapturedEvent {
            level: *metadata.level(),
            target: metadata.target().to_owned(),
            name: metadata.name(),
            message: collect.message,
            fields: collect.fields,
        });
    }
}

// ── 捕获句柄 ────────────────────────────────────────────────────────────────

/// 一段日志捕获期。drop 时自动结束。
///
/// 下面这段围栏标的是 `text` 而不是 `ignore`。差别不在渲染，在计数：`ignore` 仍然是一条
/// doctest，只是不跑；`text` 根本不进 doctest 目标。本模板要的是**一条都没有**——
/// 因为 doctest 里的 `use` 必须写 crate 真名，而真名由模板变量展开，写死任何一个都会在
/// 别人展开之后失效。"一条都没有"还顺带是条可自动检查的不变量：`cargo test --doc` 的
/// 计数必须是 0/0/0，有人加了新 doctest 立刻看得见。
///
/// ```text
/// let capture = LogCapture::start().expect("装 subscriber");
/// run_the_thing();
///
/// let announced = capture.position("shutdown_started").expect("必须公告过停机");
/// let first_exit = capture.position("task_exit").expect("必须有任务退出");
/// assert!(
///     announced < first_exit,
///     "停机公告必须先于任何取消动作\n{}",
///     capture.summary()
/// );
/// ```
///
/// 同时只能有一个存活，见模块文档「捕获期是互斥的」。
#[derive(Debug)]
pub struct LogCapture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl LogCapture {
    /// 开始捕获。前一个 [`LogCapture`] 还活着时**阻塞等待**它 drop。
    ///
    /// # Errors
    ///
    /// 全局 subscriber 已被本模块以外的代码占用时返回 [`InstallError`]。
    pub fn start() -> Result<Self, InstallError> {
        install()?;

        // 等到没人在捕获，然后把牌子挂上。
        let (lock, cvar) = &BUSY;
        let mut busy = unpoison(lock.lock());
        while *busy {
            busy = unpoison(cvar.wait(busy));
        }
        *busy = true;
        drop(busy);

        let events: Arc<Mutex<Vec<CapturedEvent>>> = Arc::default();
        *unpoison(SINK.write()) = Some(Arc::clone(&events));

        Ok(Self { events })
    }

    /// 至今捕获到的全部事件，按发生顺序。
    #[must_use]
    pub fn events(&self) -> Vec<CapturedEvent> {
        unpoison(self.events.lock()).clone()
    }

    /// 叫这个名字的全部事件，按发生顺序。
    #[must_use]
    pub fn find(&self, name: &str) -> Vec<CapturedEvent> {
        unpoison(self.events.lock())
            .iter()
            .filter(|e| e.name == name)
            .cloned()
            .collect()
    }

    /// 叫这个名字的事件出现了几次。
    #[must_use]
    pub fn count(&self, name: &str) -> usize {
        unpoison(self.events.lock())
            .iter()
            .filter(|e| e.name == name)
            .count()
    }

    /// **第一次**出现的下标。没出现过返回 `None`。
    ///
    /// 顺序断言用它。两边都要先断言存在再比较：`Option` 的比较把 `None` 排在最前，
    /// 直接比两个 `Option` 会在事件根本没出现时得到一个说谎的"通过"。
    #[must_use]
    pub fn position(&self, name: &str) -> Option<usize> {
        unpoison(self.events.lock())
            .iter()
            .position(|e| e.name == name)
    }

    /// **最后一次**出现的下标。
    ///
    /// "存储在所有任务退出之后才关"这类断言需要它：要比的是
    /// `position("storage_close_result") > last_position("task_exit")`。
    #[must_use]
    pub fn last_position(&self, name: &str) -> Option<usize> {
        unpoison(self.events.lock())
            .iter()
            .rposition(|e| e.name == name)
    }

    /// 一份适合塞进断言失败消息的摘要。
    ///
    /// 断言失败时最想知道的是"那到底记了什么"。把它写进 `assert!` 的消息里，失败一次就能
    /// 看清，不必改代码加打印再跑一遍。
    #[must_use]
    pub fn summary(&self) -> String {
        use fmt::Write as _;

        let events = unpoison(self.events.lock());
        let mut out = format!("captured {} event(s):\n", events.len());
        for (i, e) in events.iter().enumerate() {
            // 往 `String` 里 `write!` 不会失败。忽略返回值比 `unwrap` 诚实：
            // 这里没有需要处理的错误，只有一个类型上必须交代的 `Result`。
            let _ = write!(out, "  [{i}] {:>5} {}", e.level, e.name);
            if let Some(msg) = &e.message {
                let _ = write!(out, " — {msg}");
            }
            for (k, v) in &e.fields {
                let _ = write!(out, " {k}={v}");
            }
            out.push('\n');
        }
        out
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        *unpoison(SINK.write()) = None;

        let (lock, cvar) = &BUSY;
        let mut busy = unpoison(lock.lock());
        *busy = false;
        drop(busy);
        cvar.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_events_are_captured_with_their_fields() {
        let capture = LogCapture::start().expect("装 subscriber");
        tracing::info!(name: "probe_event", attempt = 3, who = "tester", "hello");

        let found = capture.find("probe_event");
        assert_eq!(found.len(), 1, "应当只捕到一条\n{}", capture.summary());
        let event = &found[0];
        assert_eq!(event.level(), Level::INFO);
        assert_eq!(event.message(), Some("hello"));
        assert_eq!(event.field("attempt"), Some("3"));
        // 字符串字段不带引号——见 `Collect::record_str`。
        assert_eq!(event.field("who"), Some("tester"));
        assert_eq!(event.field("absent"), None);
        assert!(!event.target().is_empty());
        assert!(event.fields().contains_key("attempt"));
    }

    #[test]
    fn events_from_other_threads_are_captured() {
        // 这条用例守的就是"sink 必须全局"这个选择：线程局部 sink 在这里会返回空，而本模板要断言的
        // `task_exit` / `storage_close_result` 全都由 tokio 的工作线程发出。
        let capture = LogCapture::start().expect("装 subscriber");
        let handle = std::thread::spawn(|| {
            tracing::warn!(name: "from_worker", lane = "background");
        });
        handle.join().expect("子线程不该 panic");

        let found = capture.find("from_worker");
        assert_eq!(
            found.len(),
            1,
            "跨线程事件必须被捕获，否则顺序断言会永远通过\n{}",
            capture.summary()
        );
        assert_eq!(found[0].field("lane"), Some("background"));
        assert_eq!(found[0].level(), Level::WARN);
    }

    #[test]
    fn order_is_recorded_so_sequencing_can_be_asserted() {
        let capture = LogCapture::start().expect("装 subscriber");
        tracing::info!(name: "order_first", step = 0);
        tracing::info!(name: "order_middle", step = 1);
        tracing::info!(name: "order_middle", step = 2);
        tracing::info!(name: "order_last", step = 3);

        let first = capture.position("order_first").expect("first 必须在");
        let last = capture.position("order_last").expect("last 必须在");
        let last_middle = capture
            .last_position("order_middle")
            .expect("middle 必须在");

        assert!(
            first < last_middle,
            "first 在 middle 之前\n{}",
            capture.summary()
        );
        assert!(
            last_middle < last,
            "last 必须晚于**全部** middle\n{}",
            capture.summary()
        );
        assert_eq!(capture.count("order_middle"), 2);
        assert_eq!(capture.position("never_emitted"), None);
        assert_eq!(capture.last_position("never_emitted"), None);
    }

    #[test]
    fn capture_ends_at_drop() {
        {
            let capture = LogCapture::start().expect("装 subscriber");
            tracing::info!(name: "inside_window", phase = "in");
            assert_eq!(capture.count("inside_window"), 1);
        }
        // 窗口外的事件不该被任何人收着——sink 槽位已经空了。
        tracing::info!(name: "outside_window", phase = "out");

        let capture = LogCapture::start().expect("装 subscriber");
        assert_eq!(
            capture.count("outside_window"),
            0,
            "上一段捕获结束后产生的事件不该出现在新一段里\n{}",
            capture.summary()
        );
        assert!(capture.events().iter().all(|e| e.name() != "inside_window"));
    }

    #[test]
    fn install_is_idempotent() {
        assert_eq!(install(), Ok(()));
        assert_eq!(install(), Ok(()), "重复调用必须返回第一次的结果");
    }
}
