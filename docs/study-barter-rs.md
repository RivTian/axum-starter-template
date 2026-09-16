# barter-rs 借鉴研究（Phase 1.5）

> 研究对象：`https://github.com/barter-rs/barter-rs.git`，commit `9770b27a83f844472b93b593b08063affc974b0d`（2026-01-05）。
> 规模：6 个 crate，256 个 `.rs`，约 40.7k 行。
> 取证纪律：本文写 `文件路径:行号`。理由与 `docs/audit-0913-0915.md` 相同（闸门 1 的 P4 裁决）——被引用的是**外部冻结快照**，行号不会烂，且可核查性显著更高。`docs/architecture.md` 继续只写文件路径。

## 0. 为什么值得读，以及它天然给不了什么

barter-rs 是交易引擎，不是 Web 服务。但它和本模板要解决的是同一类问题：**多个长生命周期任务 + 一个必须能干净停下来的进程 + 一组必须不互相泄漏的分层 crate**。它在这三件事上的形态有 40k 行的真实压力作为背书，值得逐条对照。

它天然给不了参考的三处，先划清楚，避免把"它没有"误读成"不需要"：

| 维度 | 它的取舍 | 证据 | 为什么对本模板不适用 |
| --- | --- | --- | --- |
| 配置 | 启动期一次性固化，无三段式管线、无热重载 | `barter/src/system/builder.rs:227-249`；全仓无 `config`/`figment`/`notify`/`watch` | 交易引擎的配置在开盘前定死是合理的；Web 服务的日志级别、限流阈值必须能在不重启的前提下改 |
| 锁文件 | `Cargo.lock` 被 gitignore | `/tmp/barter-rs/.gitignore:2` | 它是发布到 crates.io 的**库**；本模板生成的是**应用**，锁文件必须入库且 CI 用 `--locked` |
| 背压 | 全系统 unbounded channel，唯一构造器是 `mpsc_unbounded()` | `barter-integration/src/channel.rs:71`、`:174` | 它的事件速率由交易所上游决定，有天然上界；公网 HTTP 服务没有这个上界，无界队列在压力下是 OOM 而不是背压 |

---

## 1. 一句话结论

barter-rs 最值钱的东西不是某个具体类型，而是一条贯穿全仓的结构选择：**把判断集中到一个纯同步函数里，把 IO、时间、runtime 全部推到边缘并做成可注入的参数**。它的引擎核心是 `Processor<Event>::process(&mut self, event) -> Self::Audit`——无 async、无 IO、无时间（`barter/src/engine/mod.rs:75`），外面套四个约 40 行的薄 runner 适配器（`barter/src/engine/run.rs:25`）。

这与本设计为 P6 引入的 `StopPolicy` 纯状态机 + 无分支适配器（`docs/architecture.md` §12.1.1）是同一个手法。**一个 40k 行的生产 workspace 在独立演化中收敛到了同一形状**——D57/D58 因此从"我的推导"升级为"有外部先例"。

其次值钱的是它的**反面**：它的关停路径全程没有超时预算、`JoinError` 被压成字符串、abort 后不回收结果。这三条恰好是本设计投入最多篇幅的地方，它以真实代码的形式说明了不这么做的后果。

---

## 2. 采纳：会改本设计的 9 条

### S1 · `build()` / `init()` 两段分离

**它怎么做的**：装配是一条四段类型状态链，`SystemArgs`（纯数据）→ `SystemBuilder`（可选项累积）→ `SystemBuild`（已构造未启动）→ `System`（运行中）（`barter/src/system/builder.rs:70-107`、`:270-290`）。关键在于 `build()` **只做纯计算、不 spawn 任何任务**，`init()` 才启动（`:177-181`）。

**为什么采纳**：本设计 §5.2 的启动协议是"装配 → spawn → ack → 提交"一条直线，"装配失败"和"运行中失败"没有类型层的分界，只有时间顺序上的分界。分离之后，`abort_boot`（§5.2 末段）的适用范围变成一个**类型能回答的问题**——手上拿着 `Assembled` 就说明还没有任何任务在跑，清理路径不需要收割。

**落到哪里**：§5.2 拆成 `assemble() -> Assembled`（纯构造，零 `spawn`）与 `Assembled::launch(&Executors) -> Running` 两段；新增 D59。

### S2 · runtime 句柄依赖注入

**它怎么做的**：所有 spawn 走传入的 `tokio::runtime::Handle`，`init()` 只是 `init_with_runtime(Handle::current())` 的糖（`barter/src/system/builder.rs:326-338`、`:340-343`）。

**为什么采纳**：本设计已经有 `Executors`/`RuntimeSet`，但没有把"runtime 也不是隐式全局态"写成纪律。`Handle::current()` 是一个**隐式的环境读取**——它与 `std::env::current_dir()` 属于同一类东西，而后者已经被 D23 判为零出现。同一条理由应当同等适用。

**落到哪里**：新增 D60——`Handle::current()` 只允许出现在 `app` 的装配入口一处；`core`/`api`/`worker`/`storage` 内出现即红。这条和 §5.8 的零全局态是同一条纪律的两面。

### S3 · `Unrecoverable` / `Terminal` 两个正交谓词

**它怎么做的**：错误不按来源分类，按**恢复策略**分类；并且刻意用两个独立的小 trait 表达两个正交的问题——`Unrecoverable`（这个错误还能不能继续）与 `Terminal`（这个事件要不要触发关停）（`barter-integration/src/lib.rs:18-25`、`barter/src/engine/error.rs:11-18`、`:45-49`）。调用方查询谓词，而不是 match 具体变体。

**为什么采纳**：本设计的 `ExitKind`（§5.3）把"发生了什么"和"该怎么办"耦合在一个枚举里——first-failure 规则（任一退出即关停）写在散文里，不是类型能回答的。拆开之后，新增一个 `ExitKind` 变体时编译器会强迫你回答"它算不算 terminal"。

**落到哪里**：§5.3 在 `ExitKind` 旁增 `ExitKind::is_terminal()`，实现为**无 `_` 臂的 match**；新增 D61。

### S4 · 超时物化成事件，而不是丢弃

**它怎么做的**：执行请求超时后不是返回 `Err` 让上层猜，而是生成一个明确的超时事件回灌主循环（`barter/src/execution/request.rs:54`、`barter/src/execution/manager.rs:362`）。

**为什么采纳**：这正是本设计 D17「如实上报收不回来的任务」的正解形状。当前 §5.4 写的是"`unreaped` 为空"作为 `succeeded()` 的合取项之一，但没写 `unreaped` 里装的是什么。它给出了答案：装**结构化记录**，不是计数、不是日志字符串。

**落到哪里**：§5.4 的 `abort_outstanding(); reap(d2)` 残留产出 `Vec<UnreapedTask>`（含 `TaskName`、`RuntimeId`、超时发生在哪一段），进 `RunReport`；D17 的证据栏补一条断言残留记录的字段完整。

### S5 · 关停时按任务性质分类处理

**它怎么做的**：转发型任务（market/account → engine）被判定为"无需优雅关停"，直接 `abort()`；有状态组件才 `await`。这个判断被写成代码里的分类而不是口头约定（`barter/src/system/mod.rs:207-217`）。

**为什么采纳**：本设计对所有顶层任务一视同仁地 `harvest(d1)` → `abort; reap(d2)`。但 HTTP 面（有 in-flight 请求要收尾）和一个纯转发的 metrics 面在关停语义上不是一回事——给后者分配 harvest 预算是纯浪费，而预算是本设计最稀缺的资源（§5.4 的 `T` 是硬边界）。

**为什么不照抄**：它 `abort()` 之后不 await、不记录（见 §4 的 N3）。本设计的分类只决定**是否分配 harvest 预算**，不决定是否回收结果——所有任务都必须进 reap，都必须产出 `TaskExit`。

**落到哪里**：`TaskSpec` 增 `ShutdownClass::{Graceful, Abortable}`，注册时必填（无默认值）；§5.4 的 `harvest(d1)` 只等 `Graceful` 类任务；新增 D62。

### S6 · `[workspace.lints]` 集中，且 lint 集合本身值得抄

**它怎么做的**：lint 集合本体是好的（`barter/src/lib.rs:1-13`）：

```rust
#![forbid(unsafe_code)]
#![warn(unused, clippy::cognitive_complexity, unused_crate_dependencies,
        unused_extern_crates, clippy::unused_self, clippy::useless_let_if_seq,
        missing_debug_implementations, rust_2018_idioms, rust_2024_compatibility)]
```

但它把这 13 行**逐字复制进了 5 个 `lib.rs`**，且全仓没有 `[workspace.lints]`（已核实：`grep '\[lints' Cargo.toml */Cargo.toml` 无命中）。

**为什么采纳**：`unused_crate_dependencies` 这一条是意外收获——它能**自动发现 `Cargo.toml` 里的僵尸依赖**。本设计 D2 要求"每条依赖旁一句为什么"，但那只保证依赖**被解释过**，不保证它**还在被用**。一条注释写完就永远正确，删掉最后一处 `use` 之后注释仍然在。这条 lint 补上了这个缺口，而且是编译器强制的。

`missing_debug_implementations` 也值得——本设计 §7.1 已经要求 `Storage: fmt::Debug`，把它提升为全仓默认更一致。

**为什么不照抄它的三条 `allow`**：`clippy::type_complexity`/`too_many_arguments`/`type_alias_bounds` 是它 5+ 类型参数设计的**症状**（`barter/src/engine/mod.rs:104` 的 `Engine<Clock, State, ExecutionTxs, Strategy, Risk>`）。一个模板如果开局就静默这三条，会在没有它那份性能理由的前提下长出同样的编译时间问题。

**落到哪里**：根 `Cargo.toml` 增 `[workspace.lints]`，成员写 `[lints] workspace = true`；不带任何 crate 级 `allow`（与 D45「`allow` 只打在最小单元上」天然一致）。新增 D63。

### S7 · 第三方错误禁用 `#[from]`，手写 `From` 并在转换点降级

**它怎么做的**：跨层时把第三方错误**降级成 `String`**，使领域错误可 `Clone + Eq + Ord + Hash + Serialize`，从而能进事件流/审计流（`barter-execution/src/error.rs:66`、`barter-data/src/error.rs:57-61`）。

**为什么采纳**：本设计 §7.1 的 `StorageError` 已经是不含 sqlx 的自定义枚举，D34 由"`core` 不依赖 sqlx"编译期强制。但这只防住了 `core`——`storage` crate 内部完全可以写 `#[from] sqlx::Error`，然后在某次重构里把这个变体提升到门面。它给出了一条更前置的纪律：**`#[from]` 只允许用于本 workspace 内部的错误类型**，第三方一律手写 `From` 以标记降级点。

它自己在最底层违反了这条——`SocketError` 直接持有 `reqwest::Error`/`reqwest::StatusCode`/`tungstenite::Error`（`barter-integration/src/error.rs:51-62`），后果是 reqwest 大版本升级即 `barter-integration` 的 breaking change。**这是它给出的反证，不是示范**。

**落到哪里**：新增 D64——门禁扫描 `#[from]` 的目标类型路径，非本 workspace crate 即红。

### S8 · newtype 必须手写 `Deserialize` 绕道 smart constructor

**它怎么做的**：`AssetNameInternal::new` 在构造器里归一化成小写，使"未归一化的值"不可构造（`barter-instrument/src/asset/name.rs:15`）。关键细节在后面——它**手写了 `Deserialize`**，先反序列化成 `Cow<'de, str>` 再路由进 smart constructor（`:64`、`:80`）。

**为什么采纳**：这是一个真实的静默陷阱。`#[derive(Deserialize)]` 在 newtype 上会**直接构造内部字段**，完全绕过构造器的不变量。本设计 §6.3 的配置管线里有若干带约束的标量（监听地址、预算时长、日志级别），如果将来任何一个做成带校验的 newtype，`derive` 会让校验在"从文件加载"这条路径上静默失效——而这恰好是唯一真实使用的路径。

**落到哪里**：§6.3 增一条纪律 + 门禁：带 smart constructor 的 newtype 不得 `#[derive(Deserialize)]`。新增 D65。

### S9 · in-memory 实现作为真实实现的同级兄弟（闸门 1 之外的新增裁决）

**它怎么做的**：`MockExecution` 实现的是**生产 trait** `ExecutionClient`，与 `client/binance` 并列放在同一模块树下，不在 `#[cfg(test)]` 里，并且带可配置的 `latency_ms` / `fees_percent`（`barter-execution/src/client/mock/mod.rs:61`、`barter-execution/src/exchange/mock/mod.rs:40`）。配套是 `pub mod test_utils` **不加 `cfg(test)` 门控**，因而下游 crate 和 `tests/` 都能用（`barter/src/lib.rs:183`）。

**冲突**：任务书 §4 的 F3 定的是"存储单后端最小编译闭包"。加一个 `InMemoryStorage` 等于第二个 `Storage` 实现。

**裁决（人工）**：**采纳，但用 feature 门控**。`InMemoryStorage` 放在 `storage` crate 内，由 `test-utils` feature 门控，默认 feature 不编译。

**判据**：
1. F3 的字面要求是"生成结果的默认编译闭包只有一个后端"——feature 默认关闭时这一条**仍然成立**，D39 的 feature 集断言不受影响。
2. 收益是 `api`/`worker` 的契约测试不必起真实 SQLite，且**能注入存储故障**——`StorageError::Unavailable`/`Internal` 目前没有任何构造路径，这正是 §12.4 之外的又一处"变体存在但无法触发"。
3. 代价明确且有界：`storage` 多一个 cfg 分支，门禁必须多跑一次 `--features test-utils` 保证它不腐烂。这条代价写进 §11.4 而不是留给"以后注意"。
4. 它同时兑现了 §7.5 的承诺——§7.5 说"换后端时门面一行不改"，但在单实现下这句话**没有任何证据**。第二个实现（哪怕只在 feature 后面）就是那个证据。

**落到哪里**：§7.5 增第 4 条机制；§11.4 的生成结果门禁增 `--features test-utils` 一趟；新增 D66。

### S10 · 表驱动测试的零依赖形态

**它怎么做的**：局部 `struct TestCase { name, input, expected }` + `Vec<TestCase>` + 带索引的断言消息 `"TC{index} ({name}) failed"`，全仓 64 处（`barter/src/engine/clock.rs:169`）。无 `rstest`、无 `proptest`、无 `mockall`（均已确认 0 命中）。

**为什么采纳**：本设计的 `StopPolicy`、预算计算、冷热分类、错误归一化都是纯函数，正是表驱动的目标形状。它给出的是"不引第三方就能拿到 `rstest` 八成价值"的具体写法，与 D2「没理由的依赖不该在表里」一致。

**落到哪里**：模板自带一条范例（`core/src/shutdown/budget.rs` 的预算用例），`README.project.md` 点名这个写法。不新增纪律——这是风格示范，不是强制。

---

## 3. 外部印证：不改设计，但补强理由来源

| 本设计条目 | barter-rs 的独立印证 | 证据 |
| --- | --- | --- |
| D57 / D58（适配器无分支、`main()` 无判断） | 引擎核心是纯同步 `process() -> Audit`，四个约 40 行薄 runner 包在外面 | `barter/src/engine/mod.rs:75`、`barter/src/engine/run.rs:25` |
| D41（零业务全局态） | 全仓 `lazy_static`/`once_cell`/`OnceLock`/`static mut`/`thread_local!` **均 0 命中**；唯一全局副作用是日志初始化，被隔离在独立 `logging` 模块且不在装配路径上 | `barter/src/logging.rs:8-18` |
| D29（库 crate 永不装 subscriber） | 同上——`tracing_subscriber::...init()` 只在顶层 crate 的独立模块 | `barter/src/logging.rs:8-18` |
| D4（不扁平化 re-export） | 5 个 lib crate 的 `lib.rs` 里 `pub use` 数量为 **0**，全仓无 prelude；补偿手段是每个 `pub mod` 上一句职责 + `/// eg/ A, B, C` 举例 | `barter-instrument/src/lib.rs:25-40` |
| D26（`deny_unknown_fields`） | 它 **0 处**使用——对解析厂商 JSON 是对的，对解析自己的配置文件是错的。方向相反恰好确认了本设计的适用边界 | 全仓 `deny_unknown_fields` 0 命中 |
| 时间注入 | 两种形态并存：trait（`EngineClock`，`LiveClock` vs `HistoricalClock`）与闭包类型参数（`FnTime: Fn() -> DateTime<Utc>`） | `barter/src/engine/clock.rs:14`、`barter-execution/src/client/mock/mod.rs:86` |
| §4.3 跨 runtime 规则 | 同步引擎跑在 `spawn_blocking` 上、与异步侧用 channel 桥接；src 中 `std::thread::spawn` **零出现** | `barter/src/system/builder.rs:383-388` |
| D53（单一门禁入口） | 反证：它的 clippy 不带 `--all-targets --all-features`，于是 tests/benches/examples 与非默认 feature 组合**全部不过 lint** | `.github/workflows/ci.yml:88-89` |

---

## 4. 明确不采纳的 8 条（附它为此付出的代价）

| # | 它的做法 | 证据 | 为什么本设计反向选择 |
| --- | --- | --- | --- |
| N1 | 关停全程**无超时预算**：`System::shutdown()` 与 `ExecutionHandles::shutdown()` 都是无限期 `await`，`try_join_all` 无 `timeout` 包裹 | `barter/src/execution/builder.rs:363-372`、`barter/src/system/mod.rs:62-73` | 一个卡住的组件让进程永远关不掉，只能 SIGKILL。这正是审计条目 A-02 的同款缺陷。本设计 §5.4 的分层绝对 deadline 在每个 join 点包 `timeout` |
| N2 | `JoinError` 被压成字符串：`Self::JoinError(format!("{value:?}"))` | `barter/src/error.rs:43-47`（已逐字核实） | 丢失 `is_panic()` / `is_cancelled()` 的区分，恰好毁掉 §5.3 要的四分类。`ExitKind` 保留 `Panicked` / `Cancelled` 为独立变体 |
| N3 | `abort()` 后不回收结果：对所有 handle 调 `abort()` 就返回，没人 await、没人记录哪些没收回来 | `barter/src/system/mod.rs:221-227`、`:210-212` | 与 D17「如实上报收不回来的任务」直接对立。本设计 abort 之后有独立的第二段预算 reap（D16），残留进 `RunReport`（S4） |
| N4 | 关停时向 execution tx 的发送失败被静默丢弃：`let _send_result = ...` | `barter/src/engine/mod.rs:197` | 某个 manager 已死时级联就在这里断掉且无任何痕迹。D24 要求任何"失败→回落"的分支都必须留痕 |
| N5 | 公共 API 里 `expect` panic：`System::send()` 对用户可见却内部 `.expect("Engine cannot drop Feed receiver")` | `barter/src/system/mod.rs:186-188` | 引擎 panic 后任何一次 send 都连锁 panic 调用方。门面 API 必须返回 `Result` |
| N6 | `impl Iterator for UnboundedRx` 忙等自旋：`TryRecvError::Empty => continue` | `barter-integration/src/channel.rs:99-110` | 空 channel 上 100% 占满一个 blocking-pool 线程，而同步引擎正跑在 `spawn_blocking` 上消费它。真实 bug，不抄 |
| N7 | proc-macro crate（`barter-macro`，三个 derive） | `barter-macro/src/lib.rs:8`、`:44`、`:67` | 它解决的是"约 20 个零大小交易所标记类型需要相同 impl"的问题。模板没有这个 N；同样效果一个 `#[serde(rename)]` 或 10 行 `macro_rules!` 就够。代价（额外 crate、额外构建单元、`syn` 编译时间）超过收益 |
| N8 | 工程链多处名不副实：`rust-toolchain.toml` 只钉 `channel = "stable"`（浮动、不可复现）；`rustfmt.toml` 的 `imports_granularity` 是 nightly-only 选项、在其钉定的 stable 上**静默失效**；CI 用已归档的 `actions-rs/*`（2021 起停止维护）；无 `--locked`、无 `cargo deny` | `rust-toolchain.toml:2`、`rustfmt.toml:2` vs `rust-toolchain.toml:2`、`.github/workflows/ci.yml:19-22` | 逐条都是本模板应当反向钉死的项。特别是 `imports_granularity` 那条——**配置写了但不生效且不报错**，与 D54「文档只陈述能被门禁反向生成的能力」是同一类病 |

另外两条它自己的 workspace 问题，作为"模板要避开的坑"记下：`barter-integration` 的 `stream` feature 反向拉入领域 crate `barter-instrument`，造成 feature 条件下的分层逆流（`barter-integration/Cargo.toml:25`）；根 `[workspace.dependencies]` 有版本号带尾随空格的笔误 `"1.3.0 "`（`Cargo.toml:72`）。前者说明 **feature 能绕过分层纪律**——本设计 D1 的邻接表相等比较必须在**已解析 feature 集**下做，而不是在默认 feature 下做。

---

## 5. 对 `docs/architecture.md` 的修订清单

| 修订 | 章节 | 新增纪律 | 来源 |
| --- | --- | --- | --- |
| 装配拆 `assemble()` / `launch()` 两段 | §5.2 | D59 | S1 |
| `Handle::current()` 只允许出现在装配入口 | §4.3 | D60 | S2 |
| `ExitKind::is_terminal()`（无 `_` 臂） | §5.3 | D61 | S3 |
| 残留任务物化为 `Vec<UnreapedTask>` 进 `RunReport` | §5.4 | — （D17 补证据栏） | S4 |
| `ShutdownClass::{Graceful, Abortable}`，注册时必填 | §5.4 | D62 | S5 |
| `[workspace.lints]` 集中 + `unused_crate_dependencies` | §11 / 根 manifest | D63 | S6 |
| `#[from]` 只允许用于本 workspace 错误类型 | §2.6 | D64 | S7 |
| 带 smart constructor 的 newtype 禁 `#[derive(Deserialize)]` | §6.3 | D65 | S8 |
| feature 门控的 `InMemoryStorage` + 门禁多跑一趟 | §7.5 / §11.4 | D66 | S9 |
| 邻接表门禁在**已解析 feature 集**下比较 | §3.3 / D1 | —（D1 补正） | §4 末段 |
| 表驱动测试范例 | §12.1 | — | S10 |

纪律总数 58 → 66。
