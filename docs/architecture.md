# axum-starter-template 架构设计

> 阶段：Phase 1 已完成，**闸门 1 已通过**（裁决记录见 §16）；Phase 1.5（barter-rs 借鉴）已完成，差异登记见 §17。下一步 Phase 2。
> 本文是**设计文档**，不是实现记录。
> 所有"已实现""已验证"的措辞在本文中一律不出现——本阶段没有任何代码落地。
> 引用备份分支事实时只写文件路径，不写行号（行号会烂）；缺陷编号指向 [`docs/audit-0913-0915.md`](audit-0913-0915.md)。
> 引用外部冻结快照（备份分支、barter-rs）的取证台账另行保留行号，理由见 §16 的 P4 裁决。

---

## 1. 目标 / 非目标 / 模板用户的「第一天」

### 1.1 目标

一份 `cargo-generate` 模板，生成出来的项目**开箱就是一个可以进生产的 Rust Web 服务骨架**，且骨架本身把一组难写对的进程纪律写对了：

| 目标 | 具体是什么 |
| --- | --- |
| G1 单向分层的 workspace | 六个 crate，依赖边是一张可机械校验的邻接表，越界即红 |
| G2 顶层任务面可监督 | 任一顶层任务退出即进程退出；退出原因区分正常返回 / 返回错误 / panic / 被外部取消，且带任务名与所在 runtime |
| G3 关停可预测、可解释 | 固定关停序列 + 分层绝对预算；收不回来的任务如实上报，不假报 graceful |
| G4 配置可热重载且不骗人 | 启动与重载共用同一条管线；`watch` 里的值恒等于生效中的值；冷段改动整批拒绝并列出待重启清单 |
| G5 存储只能经门面访问 | 上层拿不到连接池，公共 API 不出现 `sqlx` 类型；迁移只增不改，元数据异常 fail-fast |
| G6 进程内零业务全局态 | 一个进程里能同时跑两套独立装配，互不干扰 |
| G7 模板工程链自证 | 模板仓库的门禁能证明"任意合法名字都能生成出一个自身门禁全绿的项目" |

### 1.2 非目标

| 非目标 | 理由 |
| --- | --- |
| 不带任何具体部署事实 | F6。交叉编译目标、容器链、glibc 门禁、写死端口一律不进 |
| 不带业务表、不带示例仓储、不带假成功实现 | §5.7。没有消费者的东西不预铺（见 A-18） |
| 生成结果不带 CI 配置 | F5。门禁的唯一定义是生成结果的 `make check` |
| 不做多后端存储 | F3。单后端最小编译闭包，换后端的纪律靠门面 + 门禁保证（§7.5） |
| 不承诺 Windows | 关停语义依赖 POSIX 信号。不写一个从不被编译的 `cfg` 分支（A-07 / B-15） |
| 不承诺 MSRV 兼容下限 | `rust-version` 声明的是"我们实测过的那一个版本"，不是兼容承诺（§13 DP6） |
| 不在模板里预设可观测性后端 | 指标导出、日志目的地是部署事实（§9） |

### 1.3 模板用户的「第一天」

```bash
cargo generate --git https://github.com/RivTian/axum-starter-template.git
cd <project>
make check      # 唯一的门禁入口，全绿
cargo run       # 起服务；日志第一行是 service_started，带 addr / config_source / install_root
curl localhost:<port>/readyz
# Ctrl-C：日志里每个任务都有一条收尾记录，storage=closed，unreaped_tasks=0，退出码 0
```

第一天之后要做的三件事，README 逐步骤给出，**每一步都说明"漏了会怎样、什么会变红"**：

1. 改 `[workspace.package].version` → 必须跟着跑 `make relock`（否则 `--locked` 门禁立刻红。这是 A-30 的直接教训：A 的 README 第一天第 2 步就让人把自己的门禁弄红）。
2. 加第一个仓储垂直切片（迁移 → 门面方法 → bootstrap 测试 → HTTP 端点）。
3. 加第一个顶层任务面（配置段 → 面代码返回 future → 装配层注册 → 冷热分类 → 关停预算归属）。

---

## 2. 纪律清单

> 每条：**纪律 → 为什么 → 落点（文件级）→ 自动化证据**。
> "自动化证据"一栏写的是**证据形态**，不是"已经跑过"。实际执行记录在 Phase 3 的 `docs/verification.md`。
> 括号里的编号是这条纪律的来源缺陷。
>
> **一处编号约定**：本表"为什么"一栏里出现的 `§5.x`（如 D29 的 §5.6、D35 的 §5.7）指的是
> **任务书**（`docs/prompts/axum-starter-template-redesign-agent-prompt.md`）的架构主题编号——
> §5.6 是遥测、§5.7 是存储。本文自己也有 §5.x（进程生命周期的子节），两者不是一回事。
> 本文内部的互引一律写全称（如「§7.2 唯一入口」），只有指向任务书时才用裸 `§5.x`。

### 2.1 分层与依赖

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D1 | 依赖边等于邻接表，多一条少一条都红；且比较必须在**已解析 feature 集**下做 | 分层一旦靠人守就会漂；"包含"检查放得过新增边（X-12 双后端无闭包门禁即此类）。**feature 能绕过分层纪律**：barter-rs 的 `barter-integration`（底层）在 `stream` feature 下反向拉入领域 crate `barter-instrument`，默认 feature 视角下看不见这条逆流 | `scripts/structure.sh` | 从 `cargo metadata --filter-platform` 的已解析图读实际边集，与 §3.3 的表做**相等**比较；`test-utils` 开启时（D66）另比一次 |
| D2 | 第三方版本与 feature 只在根 `[workspace.dependencies]` 声明，成员一律 `{ workspace = true }`，每条依赖旁一句「为什么」 | 版本散落即不可审计；没理由的依赖不该在表里 | `Cargo.toml` | 门禁扫描各成员 manifest：出现裸版本号即红；根表每个依赖键上方必须有非空注释行 |
| D3 | 第三方 crate 的**所在层**受限：`tracing-subscriber` 只在 `app`（允许集实际是 `{ app, testkit }`，理由见 C-142），`sqlx` 只在 `storage`，`axum`/`tower-http` 只在 `api`，`rt-multi-thread` 只在 `app` | §5.6 / §5.7 的纪律只有落到"谁能依赖它"上才有强制力 | `scripts/structure.sh::third-party` | 依赖定位断言，读的是 cargo 解析后的图而不是清单文本（C-143）；每条允许集是一个集合、相等比较，违例即红（B 的同名检查是现成形态） |
| D4 | `lib.rs` / `mod.rs` 只做 `mod` 声明与显式 `pub use`，不扁平化 re-export | 扁平化会让门面的"不泄漏"形同虚设（A-17 的 sqlx 泄漏正是从错误枚举漏出去的） | 各 crate `src/lib.rs` | 门禁扫描：`pub use` 的每一项必须出现在该 crate README 的"公共出口"清单里，两侧集合相等 |
| D5 | 整个 workspace 只有一个 `[[bin]]` | 多入口意味着多套装配，纪律会分叉 | `app/Cargo.toml` | `cargo metadata` 中 `kind == ["bin"]` 的 target 计数 == 1 |

### 2.2 任务面与进程生命周期

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D6 | 任何在 release 里会消失的检查（`debug_assert!`）都不得作为纪律载体 | A-01：重名任务只由 `debug_assert!` 拦，release 下整段被编译掉 | `core/src/task/supervisor.rs` | `register` 返回 `Result`；`duplicate_task_name_is_a_runtime_error` 在 release profile 下也跑（门禁含一次 `--release` 测试） |
| D7 | 任务名是枚举，禁止字面量比较 | B-14：改名只改一处能编译通过，关池被静默跳过 | `core/src/task/name.rs` | 门禁扫描 `.rs`：`== "http"` 形态的字符串比较即红 |
| D8 | 任务归属的 runtime 从 `Handle` 派生，不接受调用方自述 | B-02：传错标签时任务真跑在 A、报告写 B，编译与单测全过 | `core/src/task/supervisor.rs` | `spawn_on` 只接 `&Handle`；`RuntimeId` 由 `RuntimeSet` 在注册 Handle 时分配，无 setter |
| D9 | "标识缺失"与"退出原因"是两个正交字段 | B-01：元数据查不到时 panic 事实被吞成 `Invariant` | `core/src/task/exit.rs` | `TaskExit { name: Option<TaskName>, kind: ExitKind }`；`unknown_id_still_reports_the_panic` |
| D10 | 顶层任务的 future 输出类型必须能承载失败 | A-14：`Output = ()` 让"http 起不来"与"http 正常收尾"同形 | `core/src/task/mod.rs` | 类型即证据：`PlaneFuture = BoxFuture<'static, Result<(), PlaneError>>`；`serve_failure_is_classified_as_failed` |
| D11 | 编排层自己也遵守 `biased` 取消优先纪律 | A-04：全仓库唯二不 `biased` 的 `select!` 就在关停编排里 | `app/src/lifecycle/loop.rs` | 门禁扫描：`tokio::select!` 块内第一行不是 `biased;` 即红（白名单需逐条写理由） |
| D12 | 依赖 `select!` 分支顺序的正确性必须有会变红的用例 | B-37：提交循环靠 `_ = ready(())` 在最后一支，顺序被改则"准备"变"提交" | `app/src/boot/gate.rs` | 不用 `ready(())`；用显式 `StartupEvent` 优先级判定 + `commit_never_precedes_a_pending_ack` |
| D13 | 持有任务句柄的类型必须有 `Drop` 兜底 abort | A-26：`UnitManager` 无 `Drop`，外层展开即脱管 | `core/src/task/supervisor.rs` | `supervisor_drop_aborts_outstanding_tasks`（用 `Arc` 存活计数反证） |
| D14 | 空 supervisor 正常退出并留痕 | §5.2 边角；A 已有现成形态 | `app/src/lifecycle/loop.rs` | `empty_supervisor_exits_normally` |
| D59 | 装配分两段：`assemble()` 纯构造**零 `spawn`**，`launch()` 才启动任务 | 让"装配失败"与"运行中失败"成为**类型能回答的问题**：手上是 `Assembled` 就说明还没有任务在跑，清理路径不需要收割（barter-rs S1） | `app/src/assembly.rs` | 门禁扫描：`assemble` 函数体内出现 `spawn`/`spawn_on` 即红；`assemble_then_drop_leaves_no_task` |
| D60 | `Handle::current()` 只允许出现在 `app` 的装配入口一处 | runtime 是隐式环境读取，与 `std::env::current_dir()` 同类；D23 既然判后者零出现，同一理由对它同等适用（barter-rs S2） | `app/src/assembly.rs` | 门禁扫描：`core`/`api`/`worker`/`storage` 下出现 `Handle::current` 即红；`app` 内出现次数 == 1 |
| D61 | "退出原因"与"是否触发关停"是两个正交问题，后者是**无 `_` 臂**的谓词 | first-failure 规则当前只写在散文里；拆开后新增 `ExitKind` 变体时编译器强迫回答"它算不算 terminal"（barter-rs S3） | `core/src/task/exit.rs` | `ExitKind::is_terminal()` 的 match 无 `_` 臂，新增变体编译不过即证据 |

### 2.3 关停

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D15 | 关停预算是**一个绝对 deadline** 沿链传递，不是每层各拿一份同名时长 | A-11：n 个附加 runtime 下最坏 (n+1)×grace，而调用处只给了一个"3 秒" | `core/src/shutdown/budget.rs` | `Budgets::plan` 用 `checked_add` 构造累积边界；`inner_stage_never_outlives_the_outer_deadline`（属性式遍历一组预算组合） |
| D16 | abort 之后的 join 是**独立的第二段预算**，到点即放弃 | A-02：abort 后无限 `join_next` 排空；任务卡在同步段则永不返回 | `core/src/task/supervisor.rs` | `abort_reap_gives_up_at_its_own_deadline`（用 `spawn_blocking` 造不可中断任务） |
| D17 | 收割函数返回结论（是否被迫强杀 + 哪些没收回来），由调用方决定退出码；结论行必须反映真实结果 | A-03 / A-11：两种结局同为退出码 0；`info!("runtime stopped")` 超时也照喊 | `core/src/task/supervisor.rs`、`app/src/lifecycle/report.rs` | `RunReport::succeeded()` 是六项合取（B 的形态）；`forced_shutdown_reports_failure_not_success` |
| D18 | 关停期间的第二个信号 = **加速**，不刷新截止时间；并提供立即强退逃生门 | A-05：第二次 SIGTERM 被完全忽略，叠加 A-02 只能 SIGKILL | `app/src/signals.rs` | `second_stop_escalates_without_refreshing_the_timeline`；`third_signal_exits_immediately` |
| D19 | 停机公告事件先于任何取消动作 | B-13：`shutdown_started` 打在 cancel 之后，日志分段标记不是时间下界 | `app/src/lifecycle/loop.rs` | 编排层用例经 `LogCapture` 断言 `shutdown_started` 是停机段第一条事件 |
| D20 | 存储最后关；关池超时不替换原始失败 | §5.4 判据 | `app/src/lifecycle/loop.rs` | `storage_close_result` 事件时间戳晚于全部 `task_exit`；`pool_close_timeout_preserves_the_original_failure` |
| D62 | 每个顶层任务注册时必须声明 `ShutdownClass`，**无默认值**；只有 `Graceful` 类分配 harvest 预算 | 纯转发任务（如 metrics 面）没有要收尾的 in-flight 状态，给它分配 harvest 是纯浪费，而预算是最稀缺的资源（§5.4 的 `T` 是硬边界）。barter-rs S5 有现成分类，但它 abort 后不回收——本设计的分类**只决定是否分配 harvest，不决定是否回收**，所有任务一律进 reap 并产出 `TaskExit` | `core/src/task/spec.rs` | 类型即证据：`TaskSpec` 无 `Default`，字段必填；`abortable_task_gets_no_harvest_budget` |

### 2.4 配置

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D21 | 冷/热是配置类型自身的属性，新增字段不分类就**编译不过** | A-29 / X-08：前缀白名单默认判"已生效"，且回滚是第二份人工清单 | `core/src/config/heat.rs` | `classify()` 对 `Config` 做**无 `..` 的穷尽解构**；新增字段编译失败即证据 |
| D22 | 配置写入端只在装配层，且只在启动路径 | X-07：A 把内嵌模板写入端放在库 crate，且 reload 复用它——删掉配置文件后 SIGHUP 会静默重建缺省并报告成功 | `app/src/config/bootstrap.rs` | 门禁扫描：`core/` 下出现 `fs::write`/`File::create` 即红；`reload_with_missing_file_fails_and_keeps_last_good` |
| D23 | 全进程只有一个路径锚点 = 可执行文件所在目录 | B-09：cwd 与"配置文件所在目录"两套锚点；§5.9 两者都不要 | `core/src/paths.rs`、`app/src/boot/env.rs` | ①门禁断言 `std::env::current_dir` 在全 workspace **零出现**（比"试了三个 cwd"强）；②`install_root_is_the_executable_directory`、`config_and_data_are_siblings` 纯函数用例；③编排层三份不同 `exe_path` 的解析用例 |
| D24 | 任何"解析失败 / 取不到 → 回落缺省"的分支都必须留痕；已识别选项缺参数必须 fail-fast | A-10（`RUST_LOG` 写错被吞）、A-12（`--config` 无值只 eprintln）、B-07（`available_parallelism` 失败静默降级） | `app/src/telemetry.rs`、`app/src/cli.rs`、`app/src/rt.rs` | 门禁扫描：`unwrap_or`/`map_or`/`ok()` 出现在启动路径必须同文件内有对应 `warn!`（白名单逐条写理由）；`invalid_rust_log_leaves_a_trace` |
| D25 | 环境变量展开只认 `${NAME}` / `${NAME:default}`，不支持的写法一律报错 | B-05：`${A:-${B}}` 被当字面量原样吐出 | `core/src/config/expand.rs` | `nested_placeholder_is_an_error_not_a_literal` 等一组负向用例 |
| D26 | 每段 `#[serde(default, deny_unknown_fields)]` | 拼错键即错误；残留的老字段（如 runtime 的 `flavor`）当场报错而不是静默挂死（§6 坑 8） | 各配置结构体 | `residual_flavor_field_is_rejected` |
| D27 | 内嵌缺省模板逐字段等于 `Default` | §5.5 | `app/src/config/bootstrap.rs` | `embedded_template_round_trips_to_default` |
| D28 | 敏感取值优先级 `*_env > *_file > 明文`；`Debug` 渲染为 `***` | §5.5 | `core/src/config/secret.rs` | `secret_debug_never_prints_the_value`；错误信息不回显取值（B 的 `ConfigError` 是现成形态） |
| D65 | 带 smart constructor 的 newtype **不得** `#[derive(Deserialize)]`，必须手写并绕道构造器 | `derive` 直接构造内部字段、完全绕过不变量，且**静默**——失效的恰好是"从配置文件加载"这条唯一真实使用的路径（barter-rs S8 的 `AssetNameInternal` 是正解形态，它先反序列化成 `Cow<'de, str>` 再进构造器） | `core/src/config/` 下各 newtype | 门禁扫描：同一类型上同时出现 `fn new(` 返回 `Result`/`Option` 与 `#[derive(..., Deserialize, ...)]` 即红；`invalid_value_is_rejected_through_deserialization` |

### 2.5 遥测

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D29 | 库 crate 永不装 subscriber | §5.6 | — | 门禁扫描：`app/` 之外出现 `tracing_subscriber::` 即红 |
| D30 | 遥测初始化早于任何可能失败的启动步骤 | B-06：配置加载失败只能走 `eprintln!`，容器里最常见的一类失败恰是唯一看不见的 | `app/src/lib.rs`（`run` 的前三步） | 编排层用例：坏配置启动，断言失败原因以结构化事件出现而非 `eprintln!` |
| D31 | ANSI 由 `is_terminal()` 决定 | X-06：A 缺省开 ANSI，管道/文件里写入转义序列 | `app/src/telemetry.rs` | 注入 `stderr_is_terminal` 两个取值，写入内存 buffer 断言 `\x1b` 的有无——**两侧都有证据** |
| D32 | 日志格式是被下游解析的公开接口，需要快照测试 | B-35：`init()` 的输出格式改了没有任何用例会红，而门禁全部建立在它之上 | `app/tests/log_format.rs` | 对一条合成事件做逐字段快照断言 |
| D33 | 日志与错误信息里不出现 query / header / body / 配置取值 | §5.6 | `api/src/error.rs`、`core/src/config/error.rs` | 5xx 的 description 类型是 `&'static str`（编译期强制）；`config_error_never_echoes_values` |
| **D67** | **每一个**事件都显式写 `name:`（snake_case 标识符），消息位置留给给人读的那句话 | **落地时新增（C-57）**，由 C-17 收紧而来。不写 `name:` 时 tracing 自动生成的名字是 `event <文件>:<行号>`——**带行号**，而行号会烂（取证纪律点名不许依赖的东西）。退而用消息文本做断言则把两个角色压成一个：日志里没有人话，断言里没有稳定的键。C-17 原文限定「要被断言的事件」，那个条件句不可执行——没人能预先知道将来哪条事件会被用到 | 全部发事件的 crate：`core`、`worker`、`api`、`app` | 结构门禁扫 `tracing::{trace,debug,info,warn,error}!`，首参不是 `name:` 即红（C-58）；各层 README 的事件表与源码逐条对应 |

### 2.6 存储

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D34 | 门面不泄漏：成功类型与**错误类型**都不出现 `sqlx` | A-17：`StorageError::Db(#[from] sqlx::Error)` 是公共变体，sqlx 的 semver 传导到 api/app | `core/src/storage.rs`（trait + error 都在 core） | 结构门禁：`sqlx` 只能出现在 `storage` 的依赖里（D3）；`core` 不依赖 `sqlx`，编译即证据 |
| D35 | 唯一入口：连接 → 迁移 → 自检 → 返回；任一步失败先显式关池再上抛 | §5.7 | `storage/src/open.rs` | `a_failed_open_leaves_no_live_pool`：证据取**外部可见**的 WAL 边车——`-wal`/`-shm` 只在有连接挂着时存在，最后一条关掉时被删；失败之后它们还在就是漏了池 |
| D36 | 存储层不持有常驻任务 | §5.7 | `storage/src/` | 门禁扫描：`storage/` 下出现 `tokio::spawn` 即红 |
| D37 | `build.rs` 对迁移目录 `rerun-if-changed` 不可删 | `sqlx::migrate!` 对新增文件静默不生效 | `storage/build.rs` | `migration_rebuild`：对迁移的增 / 改 / 删各证明一次强制重编译（B 的形态） |
| D38 | 迁移元数据异常一律 fail-fast，不自动修复，并给出可执行修复指引 | §5.7 | `storage/src/open.rs` | 校验和不符 / 版本缺失 / dirty 三类各一条真实 SQLite 用例（B 的形态） |
| D39 | 二进制里只有一个后端，且不把 `Cargo.lock` 里的可选解析包误判为已编译 | §5.7 判据；X-12 | `scripts/structure.sh` | 从 `cargo metadata --filter-platform` 的**已解析 feature 集**判定，**逐包断言**（§18 P-A 实测字面量）：`sqlx` 含 `sqlite-bundled`/`runtime-tokio`/`migrate`/`macros` 且不含 `any`/`postgres`/`mysql`；`libsqlite3-sys` 含 `bundled`。不读 `Cargo.lock` 包列表 |
| D40 | 不预建业务表；语义错误变体必须有真实构造路径，否则不给"已验证"的外观 | A-18 | `storage/src/error_map.rs` | 归一化函数对**测试内建表**触发真实 SQLite 约束错误；无构造点的变体在 `docs/acceptance.md` 显式标「无自动化证据」 |
| D64 | `#[from]` 只允许用于本 workspace 内部的错误类型；第三方错误一律**手写 `From`**，转换点即降级点 | D34 只防住了 `core`（编译期）；`storage` 内部仍可写 `#[from] sqlx::Error`，再在某次重构里把它提升到门面。barter-rs 的 `SocketError` 正是这么泄漏的（持有 `reqwest::Error`/`tungstenite::Error`，后果是 reqwest 大版本升级即 breaking change）——**反证，不是示范** | 全仓库 | 门禁扫描 `#[from]` 的目标类型路径，首段不是本 workspace crate 即红 |
| D66 | `InMemoryStorage` 由 `test-utils` feature 门控，默认 feature 不编译；门禁必须**额外跑一趟** `--features test-utils` | ①F3 的"默认编译闭包单后端"在 feature 关闭时仍成立，D39 不受影响；②`StorageError::Unavailable`/`Internal` 目前无任何构造路径，它补上故障注入；③§7.5 声称"换后端时门面一行不改"，单实现下这句话没有证据，第二个实现就是那个证据（闸门 1 之外的人工裁决，详见 `docs/study-barter-rs.md` S9） | `storage/src/memory.rs` | `--features test-utils` 下 `cargo check` + 一组针对 `Arc<dyn Storage>` 的双实现同构用例；不跑这趟则 feature 会腐烂 |

### 2.7 装配与 HTTP

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D41 | 零业务全局态；一个进程跑两套独立装配 | §5.8 | `app/src/assembly.rs` | `two_independent_assemblies_do_not_interfere`（两个端口、两个库文件、两个 root token） |
| D42 | 错误信封覆盖路由树的补集 | A-13：顶层无 fallback，`GET /` 返回空 body 的缺省 404，而 README 说"所有对外错误都走统一信封" | `api/src/router.rs` | 打 `/`、`/nope`、`/v2/x` 的用例，断言信封三键齐全 |
| D43 | handler 必须用包装提取器，禁令由 lint 配置落地 | A-15：A 自己承认这个洞"会在第一个带请求体的 handler 上无声地打开" | `clippy.toml` | `disallowed-types` 列出 `axum::extract::{Json,Path,Query}`；`make check` 含 `clippy -- -D warnings` |
| D44 | 状态码不拍平 | DP12 | `api/src/extract.rs` | 400 / 413 / 415 / 422 各一条用例，断言彼此不相等 |
| D45 | `allow` 只打在最小单元上 | B-19：模块级 `allow(dead_code)` 让整个 `extract` 模块脱离死代码检测 | 全仓库 | 门禁扫描：模块级/crate 级 `#![allow(` 与 `#[allow(` 紧邻 `mod` 声明的形态即红 |
| D46 | 超时配置项按其真实覆盖面命名 | B-18：名为 `request_timeout`，实际只约束 handler，不约束响应体流式推送 | ~~`api/src/config.rs`~~ → `core/src/config/types.rs::HttpConfig`（C-45） | 名为 `handler_timeout`；README 与 crate 文档同时声明 body 不在覆盖面内 |

### 2.8 模板工程链

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D47 | 门禁的输入目录是模板的精确投影，增量只允许发生在 `target/` | A-32：`gen` 从不清理，删掉的文件永远留在生成目录里继续被测 | `Makefile` | `gen` 先 `rm -rf` 源码树再生成；`target/` 用独立缓存目录挂载 |
| D48 | 反向名字映射按已知成员名集合逐项替换 | A-31：无差别 `sed 's/$(GEN_PREFIX)-/…/g'`，`GEN_PREFIX=tokio` 会把 `tokio-util` 改坏 | `scripts/normalize-lock.sh` | 本地包的判据是**结构**的——`[[package]]` 块里没有 `source =` 行（见 C-147，不走 `cargo metadata`）；替换是整串带引号全等匹配，不是前缀匹配；`local-set` 先断言本地包集合 == 六个成员，`denormalize` 再断言一次性前缀零残留、第三方 `name =` 集合不变，`round-trip` 拿新锁重新生成一次过 `--locked`；负向用例 `lock_rewrite_does_not_touch_third_party_names`（前缀 `tokio` + 第三方 `tokio-util`，已实跑） |
| D49 | 名字矩阵必须含带连字符的前缀、极长名、极短名、与 crates.io 包名冲突的名 | A-33：所有门禁用的都是无连字符前缀，`crate_prefix_snake` 与 `crate_prefix` 恒等，写错变体也不可分辨 | `scripts/matrix.sh` | 四组名字各跑一次完整生成 + 结构检查，外加 `env-prefix` / `no-padding` 两条名字相关断言（C-145）|
| D50 | 至少一条门禁走用户真实的 `--git` 安装路径 | B-25：门禁全程 `--path`，`.gitattributes` 的 `export-ignore` 语义零覆盖 | `Makefile::verify-git` | 指向本地 bare 仓库生成一次并跑结构检查 |
| D51 | 未展开占位符终检匹配通用 `{{ … }}` 形态 | B-22：硬编码四个变量名，第五个变量泄漏时不会被发现 | `scripts/gen.sh`（生成即审计） | 通用正则 + 逐条写理由的豁免清单 |
| D52 | 后置钩子每一步校验结果，失败即中止生成 | B-27：三条语句都不检查返回值，失败时产物残留而生成"成功" | `hooks/post.rhai` | 结构检查的 `absent` 清单 + rhai 侧显式 `if !ok { abort(...) }` |
| D53 | 生成结果只有一个门禁入口 `make check` | F5；X-09 / B-31：B 把最有价值的门禁拆到 `check-process`，且没有任何东西会自动跑它 | `Makefile.project` | `check` 是全部子目标的并集；门禁扫描 `README.project.md` 不得出现"另外还要跑" |
| D54 | 文档只陈述能被门禁反向生成的能力 | B-20：模板 README 宣称 PTY / 文件日志覆盖，实际都没有 | `docs/acceptance.md`（264 条，九节）+ `scripts/acceptance-ids.sh` | 用例名清单与源码两侧集合比对；抽取按行锚定（C-154），判据在 `audit-rules.sh::audit_test_names`，由 `tooling-test::acceptance-extract` 喂合成输入证明它能红 |
| D55 | 验收 ID **就是**测试函数名，文档与源码两侧 ID 集合相等 | X-01：B 的 55 个验收 ID 在 `.rs` 里 0 命中，改名即索引失效——根因是那套编号与代码之间没有任何机械联系，再发一套编号只会重演 | `docs/acceptance.md` + 测试名 | `make check` 里的 `acceptance-ids`，**两个方向**都查。全树 264 个用例名唯一，所以不需要按路径限定。**不配「重新生成」的目标**：有了它两侧永远相等，而那等于没有检查 |
| D56 | 随交付物发布的文件**只讲生成项目自己的事** | B-32：模板的 `.gitignore` 带中文注释且会原样进生成项目 | `.gitignore` 等 | 门禁按显式文件清单扫模板自身的痕迹：仓库路径、备份分支名、本设计稿节号（判据按 C-67 重新定义，原文的「非 ASCII」与 F4 直接冲突） |
| D63 | lint 集合只在根 `[workspace.lints]` 声明一次，成员写 `[lints] workspace = true`；集合内含 `unused_crate_dependencies`，且**不带任何 crate 级 `allow`** | D2 只保证依赖**被解释过**，不保证它**还在被用**——注释写完就永远正确，删掉最后一处 `use` 之后注释仍在。`unused_crate_dependencies` 补上这个缺口且由编译器强制。barter-rs 的集合本体值得抄，但它把 13 行**逐字复制进 5 个 `lib.rs`** 且全仓无 `[workspace.lints]`；它那三条 crate 级 `allow` 是 5+ 类型参数设计的症状，模板开局静默它们会在没有性能理由的前提下长出同样的编译时间问题 | 根 `Cargo.toml` | 门禁扫描：任一 `.rs` 出现 `#![warn(`/`#![allow(` 即红（与 D45 同源）；各成员 manifest 必须含 `[lints] workspace = true` |

### 2.9 可测性边界（DP9 的配套硬约束）

| # | 纪律 | 为什么 | 落点 | 自动化证据 |
| --- | --- | --- | --- | --- |
| D57 | 进程边界适配器内**不得有判断**：不做预算/路径/优先级计算，控制流只允许 `?` 与单层 `if`/`match` 转发 | 生成结果不做进程测试（DP9），这两个文件因此是全仓库唯一无自动化证据的地方。只有当它们不含决策时，"没有证据"才等于"没有风险" | `app/src/boot/env.rs`、`app/src/signals.rs` | 门禁对这两个文件做语法级分支计数，越界即红 |
| D58 | 整个进程的可执行逻辑必须是一个**可调用的函数**，`main()` 不承载任何判断 | 没有进程层就只能在进程内跑编排；`main()` 里的逻辑天然不可测 | `app/src/lib.rs::run`、`app/src/main.rs` | 门禁断言 `main.rs` 行数上限且不含 `if`/`match`/循环 |

> D57/D58 有外部先例：barter-rs 的引擎核心是纯同步 `Processor<Event>::process() -> Audit`（无 async、无 IO、无时间），外面套四个约 40 行的薄 runner 适配器。一个 40k 行的生产 workspace 在独立演化中收敛到了同一形状。见 `docs/study-barter-rs.md` §1。

---

## 3. crate 划分、分层图、依赖邻接表

### 3.1 成员集合与改动理由

F1 说以 A（7 crate）为形态基线，改边要写理由。本设计定为 **6 个成员**，相对 A 删一个：

| crate | 去留 | 理由 |
| --- | --- | --- |
| `core` | 留 | 叶子；共享词汇 + 进程面原语 |
| `storage` | 留 | 单后端实现（F3） |
| `worker` | 留 | 后台任务面样板 |
| `api` | 留 | HTTP 展示层 |
| `app` | 留 | 唯一装配层与唯一 `[[bin]]` |
| `testkit` | 留 | dev-only。**理由**：§5.6 要求"测试不装全局 subscriber，日志断言用私有 subscriber 捕获"，而 tracing 的 callsite 兴趣缓存是**进程级**的，`with_default` 在并行测试下会丢事件（A 的 `testkit/src/lib.rs` 把这个坑连同噪声线程回归测试一起写下了）。这段机制若不独立成 crate，就要在 4 个 crate 的测试模块里各复制一份。它不进二进制（无人 build-depend），不进邻接表的运行期部分 |
| `reconcile` | **删** | **理由**：①它在模板里没有消费者，属于 §5.7 明令的"没有消费者的东西不预铺"；②A-24/A-25/A-26/A-27 四条缺陷全部是它**重复实现**了 `TaskSupervisor` 的取消与收敛语义，而且实现得与它自己声称"口径一致"的那份不一致（A 的 `reconcile/README.md` 说口径一致，`manager.rs::shutdown` 超时后不排空而 `supervisor.rs::wait_for_shutdown` 要排空）；③它约 1800 行，随 workspace 一起被测，给每个新项目一份既不用又必须维护的负担。删它是"少一条边"，不是"少一个能力"——期望集/生命周期骨架属于业务，由 README 的垂直切片步骤指导用户在自己的 crate 里加 |

### 3.2 分层图

```text
                    ┌─────────┐
                    │   app   │  唯一装配层 / 唯一 bin / 唯一 subscriber / 唯一写入端
                    └────┬────┘
        ┌────────────┬───┴────────┬────────────┐
        ▼            ▼            ▼            ▼
   ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐
   │ storage │  │ worker  │  │   api   │  │  core   │
   └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘
        └────────────┴────────────┴────────────┘
                          ▼
                    ┌─────────┐
                    │  core   │   叶子：不依赖任何兄弟
                    └─────────┘

   testkit ──► core            （dev-only；被 core/storage/worker/api/app 的 dev-deps 引用）
```

关键结构决定：**`Storage` trait 与 `StorageError` 定义在 `core`**，`storage` crate 只提供实现。
这样 `api` 能持有 `Arc<dyn core::Storage>` 而**不依赖 `storage`**，单向分层与门面不泄漏是同一件事的两面（直接修 A-17）。

### 3.3 依赖邻接表（门禁按此表做**相等**比较）

构建期依赖：

| from \ to | core | storage | worker | api | app | testkit |
| --- | --- | --- | --- | --- | --- | --- |
| **core** | — | ✗ | ✗ | ✗ | ✗ | ✗ |
| **storage** | ✔ | — | ✗ | ✗ | ✗ | ✗ |
| **worker** | ✔ | ✗ | — | ✗ | ✗ | ✗ |
| **api** | ✔ | ✗ | ✗ | — | ✗ | ✗ |
| **app** | ✔ | ✔ | ✔ | ✔ | — | ✗ |
| **testkit** | ✔ | ✗ | ✗ | ✗ | ✗ | — |

dev 依赖（唯一允许的额外边）：`core / storage / worker / api / app → testkit`。

**明令禁止且必须写理由才能新增的边**：

- `worker → storage`：后台任务要读写数据时，走 `app` 注入的 `Arc<dyn core::Storage>`，不是 build-depend。加这条边等于把存储实现拖进任务面。
- `api → storage`：同上。
- 任何 `→ app`：`app` 是汇点，没有出边的消费者。
- 任何 `core → *`：`core` 是叶子。
- `testkit → {storage, worker, api, app}`：夹具依赖被测层会让"测试基础设施"变成第七层。

### 3.4 各 crate 骨架内容

| crate | 内容 | 公共出口（D4 的清单，README 与 `pub use` 两侧相等） |
| --- | --- | --- |
| `core` | `paths`（安装根锚点）、`config`（类型 / 三段式管线 / 热度分类 / 展开 / secret / 错误）、`lifecycle`（`watch` 阶段广播）、`task`（`TaskName` / `TaskExit` / `TaskSupervisor` / `PlaneFuture` / `ack_channel`——回执落在这一层的理由见 C-34）、`shutdown`（`Budgets` / `Plan`）、`storage`（`Storage` trait / `StorageError` / `StorageFuture`）、`BuildInfo` | 上述各模块的类型与少量函数；无 `pub` 的自由函数做隐式初始化 |
| `storage` | `open`（唯一入口：连接 → 迁移 → 自检）、`sqlite`（池构造 / PRAGMA / 双池）、`error_map`（sqlx → `core::StorageError` 归一化）、`migrations/`、`build.rs`；`memory`（`InMemoryStorage`，`#[cfg(feature = "test-utils")]`，只用 `std`，不引入新依赖边） | `StorageOwner`（构造 + `close`），**不导出**池、不导出 `MIGRATOR`、不导出任何 sqlx 类型；`test-utils` 开启时额外导出 `InMemoryStorage`（D66，两侧清单各有一份） |
| `worker` | `ticker`（周期任务面样板：返回 future、`biased` 取消优先、热配置每 tick 现读） | `TickerPlane::{SPEC, build}`；`build(cfg_reader, lifecycle_reader, ack, cancel) -> PlaneFuture`。`SPEC` 是落地时加的（C-36）：「这个面需不需要优雅收尾」只有写这个面的人知道 |
| `api` | `router`、`state`、`extract`（包装提取器）、`error`（信封 + 状态码分档）、`response`、`system`（`/healthz` `/readyz` `/v1/info`）、`serve`（同步 `bind` + future 内 `from_std`）、`middleware`（`handler_timeout` + panic 应答） | ~~`bind(addr) -> std::net::TcpListener`、`HttpPlane::build(listener, state, cancel) -> PlaneFuture`、`AppState`~~ → **落地时修正（C-50 / C-51）**：`bind(addr) -> std::io::Result<TcpListener>`、`HttpPlane::{SPEC, build(listener, router, ack, cancel)}`、`AppState`、`router`、`HttpError`、`ENVELOPE_KEYS`、`Json` / `Path` / `Query`（共 9 项） |
| `app` | **lib + bin 两个 target**（D5 仍成立：`[[bin]]` 只有一个）。`lib.rs` 里是 `run(ProcessEnv, StopStream) -> RunReport` 与 `exit_code`；`boot/env`（进程边界唯一入口）、`cli`、`telemetry`、`config/bootstrap`（唯一写入端）、`config/reload`、`rt`（`RuntimeSet` / `Executors`）、`assembly`（零全局态装配）、`boot/gate`（启动提交门）、`lifecycle/loop`（关停编排）、`lifecycle/stop`（`StopPolicy` 纯状态机）、`lifecycle/report`、`signals`（唯一 `tokio::signal` 调用点）。`main.rs` 三行：`capture()` → `run()` → `exit_code()` | `run`、`exit_code`、`ProcessEnv`、`RunReport`、`StopRequest` —— 只为自身 `app/tests/` 的编排层用例开放（DP9 的结构前提），不供他人依赖（邻接表禁止任何 `→ app` 的边） |
| `testkit` | `LogCapture`（全局 subscriber + **全局** sink，捕获期互斥——修订见 §18.2 C-15）、临时目录 / 临时 DB 夹具、`TempInstallRoot`（造一个假的可执行文件目录） | `install()`、`LogCapture`、夹具类型 |

---

## 4. runtime 拓扑

### 4.1 默认路径

一个多线程 runtime。`worker_threads` 缺省取 `available_parallelism()`；取不到时**不静默降级**——记一条 `warn!` 并退回 1（修 B-07）。`max_blocking_threads` 可配。

### 4.2 附加 runtime

```toml
[runtime.extra.io]
worker_threads = 2
max_blocking_threads = 8

[http]
runtime = "io"        # 绑定写在面自己的段里，不是集中表

[ticker]
# 不写 runtime 键 = resolve(None) = 主 runtime
```

- **没有 `flavor` 字段**。附加 runtime 只有多线程一种形态：没有线程对 `current_thread` runtime 做 `block_on` 时，`Handle::spawn` 进去的任务永不执行（§6 坑 8）。残留的老 `flavor` 键由 `deny_unknown_fields` 当场报错（D26）。
- `resolve(None)` → 主 runtime。
- `resolve(Some(未声明的名字))` → **启动错误**。回落会把"配置写错"藏成"性能不达预期"。
- 声明了但从未被任何面引用的附加 runtime → **也是启动错误**。死配置是下一次误解的来源。
- `MAX_EXTRA_RUNTIMES` 这类"永远不可达的上限常量"不写（B-08）。真正的上限是"每个附加 runtime 必须被引用"，它自带上界。

### 4.3 跨 runtime 规则

| 规则 | 理由 |
| --- | --- |
| 共享资源（存储池、配置句柄、生命周期广播）一律在**主 runtime** 上创建，主 runtime 最后关 | §6 坑 9 |
| HTTP 面：`bind` 在启动期**同步**完成（端口占用停在启动期），`from_std` 在目标 runtime 的 future 体内完成注册 | tokio 的 IO 资源在创建时注册到当前 runtime 的驱动 |
| `RuntimeSet` 不得进入 async 上下文 | `Runtime::shutdown_timeout` 在 async 上下文 drop 会 panic（§6 坑 7）。B-03 只用注释约束，本设计用 `PhantomData<*const ()>` 让它 `!Send`，并在 `Drop` 里探测 `Handle::try_current()`：真进了 async 上下文就降级 `shutdown_background` 并记 `error!`，而不是 panic 掉整个进程 |
| 任务的 runtime 归属由 `Handle` 派生 | D8 |

### 4.4 关停顺序

见 §5.4。runtime 层面：**附加逆序、主最后**，且全部在 `block_on` 返回之后同步执行。

### 4.5 「不用多 runtime 的服务为它付出了什么」——逐项代价表

| 代价项 | 单 runtime 服务实际付出 | 怎么验证这项代价确实是这么小 |
| --- | --- | --- |
| 配置面多一个 `[runtime.extra.*]` 段 | 缺省零子表；`extra_runtimes` 为空 map | `default_config_declares_no_extra_runtime` |
| 每个面的段里多一个可选 `runtime` 键 | 不写即 `None`；解析在**启动期一次性**完成，运行期零分支 | `resolve_happens_once_at_startup`（`Executors` 构造后是 `Handle` 的只读数组） |
| `Executors` 抽象 | 面的启动签名多一个 `&Handle` 参数 | 编译即证据；无 trait object、无动态分发 |
| 关停多一段 runtime 逆序关闭 | `RuntimeSet` 的 extras 为空时是一次空循环 | `single_runtime_shutdown_visits_exactly_one_runtime` |
| `TaskExit` 多一个 runtime 字段 | 从 `Handle` 派生，单 runtime 时恒为 `RuntimeId::MAIN` | 编译即证据 |
| 线程数 | 零额外线程 | `single_runtime_spawns_no_extra_threads`（对比 `available_parallelism` 与实际线程名） |
| 编译期 | 零。没有 feature 分支，没有条件编译 | `cargo tree` 不因该能力多任何依赖 |
| HTTP 面 `bind` / `serve` 两段式 | **这一项即使永远单 runtime 也是净收益**：端口占用变成启动期错误而不是运行期日志 | `bind_failure_aborts_boot_with_the_original_error` |

### 4.6 准入清单（三种场景，且先量出来再开）

1. **CPU 密集 async 循环**：某个面的计算在 `.await` 之间连续占用 worker 线程，已量到它抬高了延迟敏感面的尾延迟。
2. **阻塞型 FFI / 同步 SDK 打满 blocking 池**：`spawn_blocking` 一旦开始就不可中断，打满后所有 `spawn_blocking` 排队。隔离的是 blocking 池，不是 worker 池。
3. **延迟敏感面需要操作系统层隔离**：需要独立线程名 / CPU 亲和 / cgroup 观测粒度。

I/O 密集的面一律不值得——tokio 的 work-stealing 已经处理它。

---

## 5. 进程生命周期

### 5.1 状态机

```text
Starting ──ack 全到齐──► Running ──stop 请求──► Draining ──► [Forcing] ──► Stopped
    │                                               │
    └──任一 ack 失败 / 取消 / 首个任务退出──► Aborting ─┘
```

方括号表示 `Forcing` 是**可跳过的**：宽限段结束时册上已经没有任务，就直接进 `Stopped`。一次干净的关停**不经过**这个阶段——它不可跳过的话就不是一个阶段，只是一行每次都发的固定日志（C-102）。

`Phase` 经 `tokio::sync::watch` 广播（`send_replace`，保证 watch 里永远持有生效值）。发布者 `LifecyclePublisher` **不是 `Clone`**——写者唯一是类型级事实。

### 5.2 启动协议（DP3：prepared ≠ committed）

装配分两段（D59）：

```rust
pub fn assemble(cfg: &Config, env: &ProcessEnv) -> Result<Assembled, BootError>;  // 纯构造，零 spawn
impl Assembled { pub fn launch(self, ex: &Executors) -> Running; }                // 唯一 spawn 点
```

`assemble()` 只构造共享资源（配置句柄、存储、生命周期广播、各面的 `TaskSpec`），**不 spawn 任何任务**。这让"装配失败"与"运行中失败"成为类型能回答的问题：手上是 `Assembled` 就说明还没有任务在跑，失败路径直接 drop 即可，不需要收割——`abort_boot` 的适用范围因此有了类型边界而不是时间顺序边界。

1. `assemble()` 构造共享资源与 `TaskSpec` 集合；每个 `TaskSpec` 必须声明 `ShutdownClass`（D62）。
2. `launch()` 对每个面 `spawn_on(spec, handle, plane_future)`。面在**完成自己的准备**之后发 `Ack`，随后**等待 `Phase::Running`** 才开始处理业务。
   ~~`spawn_on(handle, plane_future, ack_tx)`~~ —— **落地时修正（C-34）**：`spawn_on` 没有 ack 参数，`AckSender` 由 `build()` 捕获进面的 future 里。这要求 ack 类型对 `worker` 与 `api` 都可见，于是 `ack_channel` 落在 `core::task`。
   等待 `Phase::Running` 这一步必须与 `cancel` 放进同一个 `biased select!`——启动失败路径（`abort_boot`）**不发** `Draining`（C-35）。
3. ~~`Ack` 携带面特有的凭证：HTTP 面回传**真实绑定的 `SocketAddr`**（端口为 0 时这是唯一的知情途径）。~~
   **落地时取消（C-33）**：§3.4 已把 `bind(addr) -> TcpListener` 单列成 `api` 的出口，`app` 在 `launch()` 之前就同步 bind 完，真实地址从 `TcpListener` 上直接读得到——"唯一的知情途径"在同一份设计里已被推翻。`Ack` 因此是**纯信号**，面名写在接收端由装配层填。
4. 提交门循环：
   ```text
   loop {
       select! {
           biased;
           _ = cancel.cancelled()  => Abort(Cancelled)
           exit = supervisor.next() => Abort(EarlyExit(exit))   // 任一面先退出即失败
           ack  = acks.next()       => record(ack)
       }
       if acks.all_received() { break Commit }
   }
   ```
   **不用 `_ = ready(())` 兜底分支**（修 B-37）：提交条件是 `acks.all_received()` 这个显式谓词，不是分支顺序的副作用。分支顺序被改动时 `commit_never_precedes_a_pending_ack` 会红。
5. 提交：`publish(Phase::Running)`。
6. 提交后若任何路径重新读取配置，必须**重新进入"校验 → 提交"序列**（修 B-38：B 在 ack 之后用新配置重建 timer，重建失败发生在提交之后）。

启动失败路径：`abort_boot` 取消 → 短宽限收割 → 关存储 → **返回原始错误**，且不发 `Draining`（订阅者尚未就绪）。清理路径不吞首因（A 的正解）。

### 5.3 退出分类

```rust
pub struct TaskExit {
    pub name:    Option<TaskName>,         // None = 元数据缺失，与 kind 正交（修 B-01）
    pub runtime: Option<RuntimeId>,        // 由 Handle 派生（修 B-02）；缺失时与 name 一同为 None（C-01）
    pub kind:    ExitKind,
}
pub enum ExitKind {
    Returned,                  // 正常返回
    Failed(PlaneError),        // 返回错误
    Panicked(PanicSummary),    // panic（要求 unwind，见 DP2）
    Cancelled,                 // 被外部 abort
}

impl ExitKind {
    /// 「发生了什么」与「该怎么办」是两个正交问题（D61）。
    /// 无 `_` 臂：新增变体时这里编译不过，必须显式回答它算不算 terminal。
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Returned | Self::Failed(_) | Self::Panicked(_) | Self::Cancelled => true,
        }
    }
}
```

first-failure：任一顶层任务退出即进入关停。当前四个变体**都**是 terminal——但这是一个被显式写出来的结论，不是散文里的默认。空 supervisor 留痕后正常退出（D14）。

> 手法来自 barter-rs 的 `Unrecoverable` / `Terminal` 两个正交谓词（`docs/study-barter-rs.md` S3）。它同时给出了反面教材：`JoinError` 被 `format!("{value:?}")` 压成字符串，丢失 `is_panic()` / `is_cancelled()` 的区分——恰好毁掉上面这个四分类。

### 5.4 关停预算与固定顺序

```text
publish(Draining)                    ← 公告先于任何取消（修 B-13）
tracing::info!(shutdown_started)
root.cancel()                        ← 级联到所有 child token
harvest(d1)                          ← 只等 ShutdownClass::Graceful 的面自己收尾（D62）
abort_outstanding(); reap(d2)        ← 独立的第二段预算（修 A-02）；全部任务一律进这一段
storage.close(d3)                    ← 存储最后关
── block_on 返回 ──
runtimes.shutdown(d4)                ← extras 逆序、main 最后；同步执行（§6 坑 7）
```

预算构造：

```text
T  = now + total_grace                                   // 唯一的绝对边界
d1 = min(T, now + harvest_budget)
d2 = min(T, d1  + reap_budget)
d3 = min(T, d2  + storage_close_budget)
d4 = min(T, d3  + runtime_shutdown_budget)
```

- 用 `checked_add`；任一步溢出 → `Plan { invalid: true }` fail-closed（B 的形态）。
- `min(T, …)` 保证**内层严格不超外层**（修 A-11：A 是每层各拿一份 `grace`）。
- 第二次 stop 请求：`T' = min(T, now + escalate_budget)`，**保留第一个 cause 与第一个 plan**，只收紧（D18）。
- 第三次：立即强退（A-05 的逃生门）。

诚实报告：`RunReport::succeeded()` 是六项合取——cause 是信号 / 未 forced / 未 cleanup_failed / unreaped 为空 / storage 成功关闭 / **所有任务都是 `Returned`**。任何一项不成立都不算 graceful（D17）。

其中 `forced` 取自**结果**，不取自「调用过 `abort_all()`」：`reap` 段一律对剩余任务调它，而 Abortable 面按 D62 拿不到 harvest 预算，按「调用过」算的话这一项恒真、退出码恒非零（C-101）。判据因此是「被掐掉的任务 join 回来是 `Cancelled`，掐了还不回来的落进 `unreaped`」——两者都没有，说明 abort 落在了一批已经自己结束的任务上。

`unreaped` 里装的是**结构化记录**，不是计数、不是日志字符串：

```rust
pub struct UnreapedTask {
    pub name:     Option<TaskName>,
    pub runtime:  RuntimeId,
    pub class:    ShutdownClass,
    pub stalled_at: Stage,      // Harvest | Reap —— 卡在哪一段
}
```

理由：只有记录了"哪个任务、在哪个 runtime、卡在哪一段"，运维才能定位；一个数字只能告诉人"出事了"。形态取自 barter-rs 把超时**物化成事件**回灌主循环而非返回 `Err` 让上层猜（`docs/study-barter-rs.md` S4）；它自己在 abort 路径上恰好相反——`abort()` 后不 await、不记录，`JoinError` 被静默丢弃，这正是 D17 要防的东西。

### 5.5 关池的举证

存储关闭是"需要举证"的判断：只有在**可以确定没有任何面还在使用池**时才关。举不出证据就跳过关闭并如实上报 `SkippedUnproven`，而不是报 `NotCreated`（B 的正解；但 B 用 `exit.name == "http"` 字面量判定，本设计用 `TaskName` 枚举，见 D7）。

---

## 6. 配置与热重载

### 6.1 真值与热度

**真值**是 `watch` 里的那一份，并且它**恒等于生效中的配置**——这是 §5.5 的核心判据，也是 A-29/X-08 的直接反面。

热度三档：

| 档 | 含义 | 重载行为 |
| --- | --- | --- |
| 热（hot） | 每 tick / 每请求现读 | 直接生效 |
| 半热（semi） | 面启动时固化，面重启才换 | 生效于新值，但报告里列入"下次面重启生效" |
| 冷（cold） | 进程级，重启才换（线程数、监听地址、存储路径、预算） | **整批拒绝**该次重载，保留 last-good，返回变更字段清单 |

### 6.2 冷段判别机制（DP8 判据的落点）

不用"按面登记"，也不用冷前缀白名单。用**穷尽解构**：

```rust
// core/src/config/heat.rs
pub(crate) fn classify(cur: &Config, new: &Config) -> HeatDiff {
    // 无 `..`：新增字段会让这里编译不过，必须显式归档
    let Config { http, storage, worker, runtime, shutdown, telemetry } = cur;
    ...
}
```

- 新增任何字段 → 解构不穷尽 → **编译错误** → 开发者必须显式把它归到某一档。这是"漏登记时会红"的机制（§5.5 判据）。
- 冷段比较用**类型全等**（`ColdView: PartialEq + Eq`），而不是逐字段枚举：哪怕归档时只写了字段名没写比较逻辑，全等比较也会捕获它的变化。
- **报告**与**回滚**从同一次 `classify` 的结果推导，不存在第二份清单（修 X-08）。
- 一条测试遍历 `ColdView` 的派生全集断言"每个冷字段都有稳定报告路径"，清单**由派生产生**而不是手写（修 B-17）。

### 6.3 三段式管线（启动与重载共用同一条）

```text
select_path(cli, env, default)      ← 优先级：--config > <PREFIX>_CONFIG > <install_root>/config/service.toml
  ↓
read(path)                          ← 异步、可取消、有大小上限（修 B-36：不用 spawn_blocking）
  ↓  [仅启动路径] 缺文件 → 装配层写入内嵌模板（D22：写端只在 app，且 reload 不复用）
expand(${NAME} / ${NAME:default})   ← 不支持的写法报错，不按字面量放行（修 B-05）
  ↓
deserialize(serde_path_to_error)    ← 每段 #[serde(default, deny_unknown_fields)]
  ↓
resolve_paths(install_root)         ← 锚点是可执行文件目录，不是配置文件目录（修 B-09 / §5.9）
  ↓
complete()                          ← 补全
  ↓
validate()                          ← 只拦「继续跑必然失败」
  ↓
clamp()                             ← 其余越界钳位 + warn
```

`<PREFIX>` 的来源：`env!("CARGO_BIN_NAME")` 的大写形态。**Rust 源里没有任何项目名字面量**——这是 DP1 选 B 路线之后的自然结果（B 的 `main.rs` 是现成形态）。

> **实测约束（§18 P-C）**：`CARGO_BIN_NAME` **只在 bin target 内可见**。`app` 同时有 lib 与 bin 时，lib 侧 `option_env!("CARGO_BIN_NAME")` 取到 `None`。
> 因此前缀**不能**在 `app` 的 lib 里就地读取，只能由 `main.rs` 读出后作为参数向下传递：
> ```rust
> // app/src/main.rs —— 全仓库唯一出现 CARGO_BIN_NAME 的地方
> let prefix = EnvPrefix::from_bin_name(env!("CARGO_BIN_NAME"));
> ```
> 这与 D57「进程边界依赖注入」同向：前缀是进程事实，和 `ProcessEnv` 一起进 `run()`，lib 侧因此可测。

### 6.4 发布接口与重载事务

- 写端：`ConfigPublisher`（非 `Clone`，只在装配层）。
- 读端：`ConfigReader`（廉价 `Clone`），热字段每 tick 现读，跨 `await` 不持 borrow。
- 重载触发：SIGHUP。**不提供 HTTP 端点**（§9）。
- 重载执行：single-flight（同时只有一次在跑），走与启动**同一条**管线。
- 失败：保留 last-good，绝不半套用。文件缺失 = 失败（修 X-07：绝不静默重建缺省）。
- 成功且含冷段变更：整批拒绝 + 待重启清单。
- 成功且只含热/半热：`send_replace` 一次原子换值 + 生成号递增；`next.generation <= active.generation` 时抑制重复应用（B 的正解）。

### 6.5 敏感信息

`Secret<T>`：取值优先级 `*_env > *_file > 明文`；`Debug` 手写为 `***`；错误信息只回显**字段路径**，不回显取值，且字段路径截断并剥离控制字符（B 的 `ConfigError` 是现成形态）。

---

## 7. 存储

### 7.1 门面

```rust
// core/src/storage.rs —— trait 在 core，实现在 storage
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Storage: Send + Sync + fmt::Debug {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>>;
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    Unavailable { backend: &'static str },
    Migration   { stage: MigrationStage, hint: &'static str },
    NotFound,
    Conflict,
    UniqueViolation { constraint: Box<str> },
    VersionConflict,
    Internal { context: Box<str> },     // 不含 SQL、不含凭据
}
```

`Debug for dyn Storage` 只输出后端名。公共 API 与错误类型**都**不出现 `sqlx`（修 A-17）；`core` 根本不依赖 `sqlx`，这一点由编译器强制而不是由门禁追认。

### 7.2 唯一入口与关闭所有者（DP10）

```rust
// storage
pub struct StorageOwner { /* 持有池 */ }
impl StorageOwner {
    pub async fn open(cfg: &StorageConfig, cancel: &CancellationToken)
        -> Result<(StorageOwner, Arc<dyn Storage>), StorageError>;
    pub async fn close(self, deadline: Instant) -> CloseOutcome;
}
```

- 连接 → 迁移 → 自检，任一步失败 fail-fast，**失败前先显式关池**再上抛。
- 全程受 `cancel` 约束：启动期收到停止信号能中断（DP10）。
- ~~诊断路径（取根因的二次连接）**继承同一 deadline**（修 A-19：A 的 `recover_root_cause` 无超时，能挂到 OS TCP 超时）。~~
  **落地时取消（C-20）**：sqlx 0.9 的 `connect_with` 不是惰性的，当场取连接、当场把真实原因返回，根本没有"第二条连接"这一步。A-19 修的是它自己引入的问题。
- 关闭权归 `app`，在关停序列的倒数第二步（`block_on` 之内、runtime 关闭之前）。
- `CloseOutcome` 是三态 + 原因：`Closed` / `SkippedUnproven(reason)` / `Failed(err)`。**不设 `NotCreated`**——"没创建"与"没关"是两件事，合并会让 B 的那个歧义重现。
- 关池超时**不替换**原始失败（§5.4 判据）。

### 7.3 SQLite 形态

- 双池：writer（`max_connections = 1`，物理上保证单写者）+ reader（`read_only`）。
- PRAGMA 逐项显式设置并写明"默认不是决定"：`journal_mode = WAL`、`synchronous = FULL`、`foreign_keys = ON`、`busy_timeout`。
- `acquire_timeout` 显式设置（不用 sqlx 默认的 30s——排队会成为故障放大器）。
- 关闭顺序：先 reader 后 writer（WAL 的原因写在注释里）。
- 文件权限 0600，覆盖 `-wal` / `-shm` 两个边车文件。
- 数据文件位置：`<install_root>/data/`，由 §5.9 的锚点派生。

### 7.4 迁移

- 只增不改；已发布不可动。
- `storage/build.rs` 对迁移目录 `rerun-if-changed`，**不可删**，理由写在 `storage/migrations/README.md` 里（D37）。
- 元数据异常（校验和不符 / 版本缺失 / dirty）**一律 fail-fast，不自动修复**，并给出可执行的修复指引（"在哪、怎么修"）。
- ~~`MAX_MIGRATION_VERSION` 查询带 `WHERE success = TRUE` 过滤。~~
  **落地时取消（C-32）**：模板里没有这个查询的任何消费者——`/v1/info` 给构建信息，门面上也没有
  对应方法。预铺一条没人调用的查询正是 §7.5 自己禁的事。那条过滤的**理由**移进
  `storage/migrations/README.md`，留给第一个真需要它的人。
- 模板自带的迁移集**不创建任何业务表**，并有一条会主动过期的断言：`migration_set_creates_no_business_tables` 直连原库核对表清单（绕开被测门面），注释写明"第一条业务迁移落地时把这里从 `None` 改成 `Some(1)`"。

### 7.5 单后端闭包下怎么保证"换后端时门面不泄漏"

A 的"绝不单后端落地"纪律（靠双后端互相牵制来暴露泄漏）在 F3 之下失去了载体。替代机制是三条**互不依赖**的约束：

1. **类型位置**：`Storage` trait 与 `StorageError` 在 `core`，而 `core` 不依赖任何数据库 crate。泄漏一个 `sqlx` 类型到门面 = `core` 要 build-depend `sqlx` = 邻接表门禁（D1/D3）直接红。这是编译期的、不可绕过的。
2. **编译闭包门禁**（D39）：从 `cargo metadata --filter-platform` 读**已解析的 feature 集**，**逐包**断言——`sqlx` 含 `sqlite-bundled`/`runtime-tokio`/`migrate`/`macros`、不含 `any`/`postgres`/`mysql`；`libsqlite3-sys` 含 `bundled`。**不读 `Cargo.lock` 的包列表**——锁文件里的可选解析包并不等于被编译进二进制，读它会误判（§5.7 判据点名的那件事）。
   注意 `bundled` **不是 `sqlx` 的 feature**，而是 `libsqlite3-sys` 的（§18 P-A）。把两者写在同一个断言里会永远为假，门禁形同虚设——这正是「断言字面量必须实测」的理由。
3. **公共出口清单比对**（D4）：`storage` crate 的 `pub use` 集合必须等于其 README 的"公共出口"清单。新增一个导出而不改文档即红；改了文档就有人会看到它导出了什么。
4. **第二个实现**（D66）：`InMemoryStorage` 由 `test-utils` feature 门控，默认不编译。前三条都是"泄漏了会红"的**否定式**证据；这一条是唯一的**肯定式**证据——门面确实支撑得起两个形态完全不同的后端。

第 4 条是闸门 1 之后追加的裁决。它与 F3「单后端最小编译闭包」不冲突：F3 约束的是生成结果的**默认编译闭包**，feature 关闭时该约束仍然成立，D39 的 feature 集断言不受影响。代价是 `storage` 多一个 cfg 分支、门禁必须多跑一趟 `--features test-utils`（否则这个 feature 会腐烂），这条代价写进 §11.4 而不是留给"以后注意"。收益除了举证之外还有一项：`StorageError::Unavailable` / `Internal` 此前**没有任何构造路径**，有了内存实现才能注入故障去验证上层的降级行为。完整论证见 `docs/study-barter-rs.md` S9。

换后端的操作被定义为：新增 `storage/src/<backend>.rs`、把 `open` 的分派换掉、更新 D39 的 feature 断言。**门面一行不改**——如果需要改门面，就说明泄漏已经发生。

---

## 8. HTTP 契约

### 8.1 最小状态

```rust
pub struct AppState {
    pub storage:   Arc<dyn core::Storage>,
    pub lifecycle: LifecycleReader,
    pub config:    ConfigReader,
    pub build:     BuildInfo,
}
```

零全局态；`Router` 由 `AppState` 构造（§5.8 / D41）。

### 8.2 系统端点

| 端点 | 语义 | 判据 |
| --- | --- | --- |
| `GET /healthz` | 存活。进程还能应答就 200 | 不查存储；不因 draining 变化 |
| `GET /readyz` | 就绪。`Phase == Running` **且** `storage.health()` 成功才 200 | 其余返回 503 + 原因短名（~~`starting` / `draining` / `storage_unavailable`~~ → **落地时扩张（C-49）**：`Phase::as_str()` 全集 ∪ `storage_unavailable`）。**跨越 draining 的探测一律不 ready**——否则负载均衡会在关停窗口里继续送流量 |
| `GET /v1/info` | 构建信息 + uptime | 三个字段 `service` / `version` / `uptime_seconds`。前两个取自 `core::BuildInfo`，测试断言的来源必须是同一次调用（修 A-16：A 断言的是 api crate 的 `CARGO_PKG_VERSION`，只因所有成员版本恰好相等才通过）。`service` 是**可执行文件**的名字，一路来自 `main.rs` 的 `env!("CARGO_BIN_NAME")`——不是任何一个 crate 的 `CARGO_PKG_NAME`，那是 workspace 的内部构造，发到对外契约上等于把仓库布局公开（C-103 / C-105） |

`/readyz` 自身的存储探测**有界**：`handler_timeout` 覆盖它，超时即 503 而不是挂住。

### 8.3 响应与安全契约

- 统一错误信封，且**覆盖路由树的补集**：~~`nest` 内 fallback + **顶层 fallback**~~（修 A-13）→ **落地时收缩（C-47）**：只留**顶层 fallback**。实测内层路由器没有自己的 fallback 时会继承外层的，内层那条是多余的；而给 `nest` 进去的路由器**加**了 fallback 反倒会让顶层那条对整个子树失效——原文把安全方向和危险方向写反了。模板里因此一条 `nest` 都没有。另补**第六个漏点**：方法不对时 axum 回 405 + 空 body，由 `method_not_allowed_fallback` 接管（C-48）。用例要打 `/`、`/nope`、`/v1/nope`、`/v1/info/extra`、`POST /healthz`、`/boom`。
- 成功响应是裸 JSON（无信封）。契约测试断言的是**信封三个键一个都不在**——只断业务字段在的话，套了信封也照样过（A 的正解）。
- 5xx 的 description 类型是 `&'static str`：用户输入在编译期就进不去（B 的正解）。
- **不写 `From<StorageError> for HttpError`**：模板里没有业务仓储，任何映射都只能产出 500，写出来是零信息的仪式，还会给"已验证"的错觉（A-18）。取而代之：`api/src/error.rs` 里有一个**无 `_` 臂**的 `match`，覆盖 `StorageError` 全部变体 → 状态码。新增变体则编译不过。`VersionConflict` 在模板内**无构造点**，`docs/acceptance.md` 对它显式标「无自动化证据」，不用测试伪装。
- 错误来源沿 `source()` 逐层 downcast 还原语义，而不是只匹配顶层（A 的正解：`#[from]` 顶层匹配会让 404/409 在加过 `Context` 之后静默退化成 500）。

### 8.4 提取器与状态码（DP12）

包装提取器预铺并由 lint 强制（D43）。状态码**不拍平**：

| 情形 | 状态码 |
| --- | --- |
| 路径参数类型不符 | 400 |
| JSON 语法错误 | 400 |
| Content-Type 不是 JSON | 415 |
| body 超限 | 413 |
| JSON 结构正确但语义不符（缺字段 / 类型错） | 422 |
| **落地时补入（C-46）**：handler 的 `Path<T>` 元数与路由参数个数对不上 | **500** |

四条用例各断言一次，并断言**彼此不相等**（防止将来被谁统一成 400）。

**落地时修正（C-46）**：这张表不在 `api` 里重抄一遍，包装提取器直接委托 axum rejection 的
`.status()`。两条理由：①上表各档恰好就是 axum 0.8.9 的默认值，抄一份只是多一份会漂移的副本；
②**axum 的分档比这张表细**——`FailedToDeserializePathParams` 把「路径参数类型不符」判 400、
把「元数对不上」判 **500**，后者是程序员写错了而不是客户端发错了。手抄表必然把两者压成同一个
400，于是一个自己写错的 bug 会被报成客户端错误，在监控上永远查不出来。

### 8.5 就绪语义与 graceful 边界（DP11）

- `bind` 在启动期**同步**完成 → 端口占用是启动错误。
- ~~`Ack` 回传真实 `SocketAddr` → 端口 0 时调用方也知情。~~ **落地时取消（C-33）**：既然 `bind` 在启动期同步完成，`app` 手上就有 `TcpListener`，`local_addr()` 直接给出真实地址——端口 0 的知情途径本来就在上一条里。`Ack` 退回纯信号。
- **绑定 ≠ 就绪**：~~面在收到 `Phase::Running` 之前不处理业务~~（B 的 `listener_binding_does_not_open_the_startup_commit_gate` 是现成的契约测试形态）→ **落地时改写（C-52）**：这件事由 `/readyz` 在 `Running` 之前回 503 实现，**不**由推迟 accept 实现。推迟 accept 会让健康检查自己也连不上，编排器读到的是「连接被拒」而不是「已绑定但未就绪」——两者在排障时完全不同，而后者正是就绪探针存在的意义。
- `axum::serve(...).with_graceful_shutdown(root.cancelled())`。
- 中间件顺序（外 → 内）：`TraceLayer` → `CatchPanicLayer` → `handler_timeout` → `DefaultBodyLimit`。每层为什么在那个位置写在注释里；重排会同时破坏 trace 覆盖与 panic 捕获。
- `handler_timeout` **不覆盖响应体的流式推送**——名字和文档都这么说（修 B-18）。
- **`handler_timeout` 不用 `tower_http::timeout::TimeoutLayer`，自己实现**（§18 P-D 实测改判）。两条独立理由：
  1. tower-http 0.7.1 的超时响应是 `Response::new(B::default())`——**空 body、无 `content-type`**。它绕过 §8.3 的统一错误信封，而 §8.3 要求信封覆盖路由树的补集。用它就等于给契约开一个无人看守的洞。
  2. `TimeoutLayer::new` 自 0.6.7 起 `#[deprecated]`，而 D63 的门禁是 `-D warnings`——照 A 的写法直接红。
  落点 `api/src/middleware/timeout.rs`，形态：
  ```rust
  // 只包裹「产出响应」的 future，不包裹 body 流——B-18 的修复因此是结构性的，不是文档承诺
  let fut = next.run(req);
  match tokio::time::timeout(budget, fut).await {
      Ok(resp) => resp,
      Err(_elapsed) => HttpError::Timeout.into_response(),   // 走信封
  }
  ```
  **落地时修正（C-53）**：`HttpError` 是**结构体**不是枚举（字段全私有，构造途径只有
  `client` / `server` 两条），上面那一行实际写作 `HttpError::timeout()`。枚举形态会把
  「哪个变体配哪个状态码」摊到公共 API 上，于是任何人都能构造一个 5xx 并塞进任意 `String`
  ——而 5xx 的 message 必须是 `&'static str` 正是靠 `server` 的参数类型保证的。

  依赖面不变：只用 `axum` + `tokio/time`，二者已在 `api` 的准入清单内（D3）；`tower-http` 因此**不开 `timeout` feature**。
- 超时状态码取 **504 Gateway Timeout**，不取 tower-http 弃用默认值的 408。理由：RFC 9110 §15.5.9 的 408 指「服务器未在等待窗口内收到完整请求」，是把责任归给客户端；而这里超时的是**本服务自己的 handler**，归因写反会误导调用方重试策略。504 同样不精确（我们不是网关），但它至少不诬告对端。这条差异必须写进 `api` 的注释，否则下一个人会"修"回 408。

---

## 9. 横切能力准入清单

| 能力 | 裁决 | 理由 |
| --- | --- | --- |
| 结构化日志（tracing + `EnvFilter`） | **内置** | 没有它则 D30/D31/D32 无处落脚 |
| 请求 trace 层（`TraceLayer`） | **内置** | 请求级 span 是排障基线；已在依赖表里，零新增 |
| panic 捕获层（`CatchPanicLayer`） | **内置** | 与 DP2 的 unwind 选择配套：一个 handler panic 不该杀掉进程 |
| handler 超时层 | **内置** | 无界 handler 会让关停预算失效 |
| body 大小上限 | **内置** | 无上限的 body 是最便宜的一类 DoS 面；默认值保守并可配 |
| 系统端点（`/healthz` `/readyz` `/v1/info`） | **内置** | §8.2 |
| SIGHUP 配置重载 | **内置** | §5.5 要求热重载，SIGHUP 不引入任何外部面 |
| CORS | **不进** | 允许的 origin 是部署事实（F6）。预铺一个 `Any` 是安全倒退，预铺一个空清单是零价值 |
| OpenAPI（utoipa 等） | **不进** | 需要在每个 handler 上加宏属性，模板里没有业务 handler 可加；且会把一个大依赖钉进每个新项目。**两分支都没有它**，没有任何既有证据支持准入 |
| Prometheus / metrics 导出 | **不进** | 指标后端是部署事实（F6）。`/v1/info` 已给出 build + uptime 这类零依赖的进程事实 |
| HTTP 客户端（reqwest 等） | **不进** | 模板里没有消费者。"没有消费者的东西不预铺"（A-18 的教训） |
| 文件日志（tracing-appender） | **不进** | 日志目的地是部署事实：容器与 systemd 都从 stdout 收。B 的模板 README 宣称有文件日志覆盖而实际没有（B-20/B-35），这是反面证据 |
| 配置 reload HTTP 端点 | **不进** | 把重载暴露成外部面就需要鉴权，而鉴权方案是部署事实。SIGHUP 已覆盖同一需求且不新增攻击面 |
| 认证 / 鉴权 | **不进** | 同上。README 的垂直切片步骤说明在哪一层加 |

"不进"的每一条都在 `README.project.md` 的「当前边界」一节列名并说明在哪加——**不进**不等于**不提**。

---

## 10. 部署布局

```text
<install_root>/                  ← = 可执行文件所在目录，永远不取 cwd
├── <bin>
├── config/
│   └── service.toml             ← 运维改
└── data/
    ├── service.sqlite3          ← 进程写
    ├── service.sqlite3-wal
    └── service.sqlite3-shm
```

- 锚点 = `current_exe()` 解析并规范化后的父目录。
- `--config` / `<PREFIX>_CONFIG` **只改读哪个文件**，不改锚点。配置里所有相对路径（含 `data/`）都从锚点派生（修 B-09：B 的相对路径锚在配置文件所在目录）。
- config 与 data 是两个**同级**目录：运维改 / 进程写，权限与备份策略不同。
- **结构性排除**"换个目录启动读到另一份配置"：不靠"试几个 cwd 看结果一样"，而是让 `std::env::current_dir` 在整个 workspace 里**一次都不出现**，由门禁逐字节证明（D23）。进程的 cwd 根本没有进入任何决策，因此不需要枚举它的取值。
- 开发期的直接后果：`install_root` 是 `target/debug/`，配置在 `target/debug/config/service.toml`。这不是缺陷，是纪律的代价，`README.project.md` 明写并给出 `make dev-config` 之类的一键准备。
- **不带**：端口默认值以外的任何网络事实、容器文件、systemd unit、交叉编译目标、glibc 门禁（F6）。

---

## 11. 模板生成与工程链

### 11.1 从 A/B 继承的形态（F2 的「形态」）

| 形态 | 来源 | 继承理由 |
| --- | --- | --- |
| 生成目录在仓库树**外** | A `Makefile` | cargo-generate 对 `--path` 模板是先整目录复制再套 `ignore`；放树内会让 git status 与工具都得绕着它走 |
| `make gen` / `check` / `verify` / `lock` / `clean` 五目标骨架 | A `Makefile` | F2 |
| `make verify` = `cargo generate --test`，**换一组名字、不复用 target、走 cargo-generate 自己的展开路径** | A `Makefile` | 三处不重复的价值 |
| `lock` 写回模板 `Cargo.lock` | A `Makefile` | §6 坑 11 |
| `structure()` 把架构约束变成可执行断言 | B `scripts/template.py` | §5.1 判据的现成形态 |
| 名字矩阵 | B `scripts/template.py` | A-33 的洞由它补 |
| 迁移重编译证明 | B `scripts/template.py` | D37 |
| 破坏性操作三重护栏（拒绝 ROOT / `$HOME` / `/`，symlink + 前缀 + ownership marker + `flock`） | B `scripts/template.py` | 门禁会 `rm -rf` |
| `hooks/pre.rhai` 身份校验 | B | A 的 pre hook 没有任何校验 |
| `hooks/post.rhai` 改名 + 自删（`hooks/` 不能进 `ignore`） | A/B | §6 坑 3 |

### 11.2 必须改造项（逐条对应审计条目）

| 改造 | 修的是 |
| --- | --- |
| `gen` 先清源码树再生成 | A-32 |
| `normalize_lock` 只映射 `cargo metadata` 的本地包，不做前缀通配 | A-31 |
| 名字矩阵含带连字符前缀 / 极长 / 极短 / 与 crates.io 冲突四组 | A-33 + DoD 3 |
| `include` 白名单用 `**/Cargo.toml` 递归 | B-26 |
| 未展开占位符终检匹配通用 `{{ … }}` | B-22 |
| 未交付文档引用检查以 `docs/` 实际不发布的文件名集合动态生成 | B-21 |
| 全量文件扫描先判文本/二进制 | B-23 |
| 删掉 `m\d+` 全面禁用的历史标记正则，改为限定真实里程碑形态 + 逐行豁免 | B-24 |
| 增加一条 `--git` 路径的生成 + 结构检查 | B-25 |
| `post.rhai` 每步校验返回值，失败即 `abort` | B-27 |
| `Makefile.project` 的 `check` 是全部子目标的并集，单一入口 | X-09 / B-31 |
| 所有依赖外部前置条件的目标声明 `preflight` 依赖 | B-28 |
| `.gitattributes` 覆盖参与字节比较的全部文件类型（含 `*.md`、`Cargo.lock`） | B-33 |
| 每个门禁入口输出它实际覆盖的检查项清单 | B-34 |
| `.gitignore` 等随交付文件改用英文 | B-32 |
| 精确钉版则配审计门禁 —— 本设计改用 semver + 锁文件（§13 DP5），审计门禁改为模板仓库 CI 的定期 `cargo audit` | B-29 |
| `cargo_generate_version` 用兼容区间 | B-30 |

上表里 A-31 与 B-21..B-24 这五条改的都是**判据本身**，而判据出问题的表现和判据通过是同一个样子：一条永远匹配不上的正则、一份路径写错了的豁免、一个把第三方包名也换掉的替换，输出全是绿的。所以这五条各自配一个负向用例，落在 `scripts/tooling-test.sh`（§11.4 的 `tooling-test` 行逐项列出）。判据本身从 `gen.sh` 里搬进 `scripts/audit-rules.sh` 就是为了这件事——只定义、不执行，于是真实的树和合成输入用的是同一份定义，而不是一份会往「永远绿」方向漂移的复制品。

### 11.3 渲染面（DP1 的落点）

```toml
# cargo-generate.toml
[template]
cargo_generate_version = ">=0.24, <0.25"
include = ["Cargo.toml", "**/Cargo.toml", "Cargo.lock", "README.project.md", "Makefile.project"]
ignore  = ["docs", "scripts", "target", ".github", "Makefile", "README.md", ".genignore"]

[placeholders]
crate_prefix = { type = "string", regex = "^[a-z][a-z0-9]*(-[a-z0-9]+)*$", prompt = "..." }

[hooks]
pre  = ["hooks/pre.rhai"]
post = ["hooks/post.rhai"]
```

> 上面这段 TOML 是设计期的草案，**已被实测修订取代**：`include` / `exclude` 管的是渲染而不是收录（C-112），`ignore` 收的是字面路径且拼错不报错（C-113），`.genignore` 那条多余（C-116），`**/README.md` 与 `rust-toolchain.toml` 漏了（C-117），`[placeholders]` 整条换成了 pre 钩子里的推导（C-118），下面那条「`hooks/` 在 `post.rhai` 里自删」是错的（C-115）。以交付的 `cargo-generate.toml` 为准，本段留作设计记录。

- **`.rs` 一律逐字节复制**，零占位符。
- 项目名靠根 manifest 的 `package =` 别名挡在 Rust 源之外：
  ```toml
  [workspace.dependencies]
  service-core = { package = "{{crate_prefix}}-core", path = "core" }
  ```
  Rust 源里永远写 `service_core::…`。
- 于是 rustfmt 的名字依赖**从结构上消失**：`.rs` 的字节在生成前后完全相同，`fmt-portable` / `fmt-matrix` / `template-sync.py` 三件套（A 的两个 Python 脚本 + 两条门禁）全部失去存在理由。这是 DP1 选 B 路线的主要收益。
- `hooks/` 不进 `ignore`，在 `post.rhai` 里自删（§6 坑 3）。
- `.github` 进 `ignore`：生成结果不带模板自己的 workflow（F5 / §6 坑 5）。

### 11.4 两类门禁

**模板仓库的 `make check`**（单一入口）：

| 子目标 | 内容 |
| --- | --- |
| `preflight` | 工具链 / cargo-generate / `git` 版本检查，失败即红且给出诊断（**不含 Python**，见 C-111） |
| `tooling-test` | `scripts/` 自身的单元测试：把每条判据喂上**合成输入**，两边都验——本该逮住的确实被逮住，不该逮住的确实放过。9 项：`placeholder-generic`（B-22，含一个从未声明过的变量名）、`placeholder-exempt`（豁免按完整路径全等，不退化成按文件名）、`history-shape`（B-24，七条 catch + 六条 pass）、`trace-allows-rfc`、`undelivered-computed`（B-21，`Makefile` 撞基名要滤、`Makefile.project` 要留）、`binary-skipped`（B-23，带 NUL 的资产里埋一条历史标记）、`lock-third-party-kept`（A-31，前缀 `tokio` + 第三方 `tokio-util`）、`acceptance-extract`（D54 / C-154，六种可达形态全抽到——含带参数的 `#[tokio::test(…)]`；散文里的 `#[test]`、夹空行、夹代码、被注释掉的属性、`#[test_case]`、跨文件残留全部不算；属性被折成多行时当场非零退出）、`prefix-collision`（C-144，真跑一趟 `cargo generate --name axum`，要求它红）。落点：`scripts/tooling-test.sh` |
| `acceptance-ids` | `docs/acceptance.md` 的 ID 集合与源码用例名集合**完全相等**（D54 / D55）。3 项：`doc-shape`（条目形态合法且无重复认领）、`ids-equal`（**两个方向**都查——只查文档→源码挡得住假条目，挡不住漏声称）、`section-counts`（节标题里的数量与实际条目数一致）。抽取判据与 `tooling-test` 共用 `audit-rules.sh::audit_test_names` 这一份定义。**故意没有「重新生成这份文档」的目标**：有了它，所有人都会先跑它再看红绿，于是两侧永远相等，而「永远相等」和「没有检查」是同一件事。排在 `gen` 前面是因为它纯文本比对、最便宜，而改完代码最容易忘的就是它。落点：`scripts/acceptance-ids.sh` |
| `gen` | 树外生成默认工程；**生成即审计**（未展开占位符、历史标记、未交付文档引用、随交付文件语言） |
| `structure` | §3.3 邻接表相等比较、依赖定位（D3 / C-142）、单 bin、sqlx feature 闭包（D39）、`absent` 清单与 `assert_absent` ↔ `ignore` 交叉比对、撞名前缀清单与锁文件反推结果交叉比对（C-144）、非渲染文件逐字节相等、模板树无符号链接（C-121）、生成树垃圾递归扫（C-139）。**公共出口清单比对（D4）不在这里**：C-127 把它落进了生成结果自己的 `testkit/tests/discipline.rs`，同一条纪律写两遍只会改一边 |
| `project-check` | 在生成工程里跑它自己的 `make check`，并**读日志证明它跑全了**。5 项：`default-goal`（裸 `make -n` 打 help 而不是启动构建）、`project-gate`（绿，且 `fmt`/`lint`/`test` 三条分隔行与通过行都在）、`storage-feature`（D66 的第二趟真跑了——判据是一个只有 `test-utils` 打开才编译得出来的测试名，而不是 Makefile 里那句 echo）、`doctest-zero`、`cache-shared`（产物落在共用缓存、生成树里不留 `target/`）。这五条的共同前提是**退出码 0 证明不了「跑了什么」**：`check: fmt lint test` 掉一条依赖、D66 那趟被删、doctest 那段 `if` 被改成永真，三种改动都只让输出变短，退出码不变。它不自己生成，消费 `gen` 留下的同一棵树——各自生成一次的话，`structure` 验过的树和这里跑 `make check` 的树就成了两棵。落点：`scripts/project-check.sh` |
| `matrix` | 四组名字（带连字符 / 50 字符 / 单字符 / 与知名 crate 同名但不撞锁）各走一遍 `gen` + `structure`，另加两条只有换名字才可能红的断言：环境变量前缀的渲染面 ↔ 运行期相等、渲染面无「项目名后补空格对齐」（C-145）|
| `migration-rebuild` | 迁移增 / 改 / 删各证明一次强制重编译，外加一条 `baseline`（无改动时**不**重编，先证明这个观察能变绿）与一条负向对照（拿掉 `storage/build.rs` 后新增迁移被静默忽略）。落点：`scripts/migration-rebuild.sh`，5 项 |
| `release-probe` | 真实 release 产物的 panic 可观察性：构建并运行一个注入的 example，逐行比对它的 stdout（`release-artifact` / `panic-classified` / `process-survives`），再把根清单的 profile 改成 `panic="abort"` 要求编译期守卫必须红（`abort-guard`）。落点：`scripts/release-probe.sh`，4 项 |

`make verify` 与 `make verify-git` 是 `check` 之外的两条独立入口，CI 各跑一次：

| 入口 | 内容 | 落点 |
| --- | --- | --- |
| `verify` | cargo-generate 的**第三条展开路径** `--test`，4 项：`cold-target`（`CARGO_TARGET_DIR` 未设置，从零编——断言必须在跑之前下，跑完再断言等于什么都没验）、`expand-test`、`random-name`（项目名来自 cargo-generate 自己的随机词表，且不得撞上门禁挑过的七个名字之一——撞上说明 `--test` 不再随机取名，这一趟会退化成又一遍名字矩阵）、`project-gate`。它先把模板树复制到 `$GATE_ROOT` 再跑：`--test` 展开的是 `$CWD` 且会在 `$CWD` 里建一个随机名目录，直接在模板根上跑等于每次往仓库扔一个目录，而模板仓库的写入口只能是 `make lock` | `scripts/verify.sh` |
| `verify-git` | 真实的 `--git` 安装路径，4 项：`git-path`（委托 `gen.sh --git`，审计五项随之跑完）、`crlf-neutral`（克隆方 `core.autocrlf=true` 时生成树与常规那趟逐字节相同）、`crlf-negative`（**负向对照**：复制快照仓库、删掉 `.gitattributes`、同一趟必须出现 CRLF——实测 89 个文件变脏）、`absent-via-git`（§15.2 里「`--git` 模式下 `ignore` 与 `hooks/` 自删的时序」那条的落点，结构检查第一次在克隆树上跑） | `scripts/verify-git.sh` |

`crlf-negative` 那条是这两个入口里唯一一条**只为别人而存在**的断言：没有它，`crlf-neutral` 的绿有两种解释——`* -text` 起了作用，或者这台机器根本不做换行转换，而两者在输出上一模一样。C-141 给 `.gitattributes` 写的那条理由，只有在负向对照成立时才是一条被证过的理由。

**生成结果的 `make check`**（F5：门禁的唯一定义）：

```make
check: fmt lint test   # 三条门禁（§9 的规格），并集，README 不得说「另外还要跑 X」
```

- `fmt` = `cargo fmt --check`
- `lint` = `cargo clippy --workspace --all-targets --locked -- -D warnings`（含 `clippy.toml` 的 `disallowed-types`，D43）
- `test` = `cargo test --workspace --locked`，其中 `app/tests/` 是进程内的编排层场景（DP9）
- `test` 内含**第二趟** `cargo test -p <prefix>-storage --features test-utils --locked`（D66）。不跑这趟，`InMemoryStorage` 会在默认 feature 下静默腐烂——一个永远不编译的实现举不了任何证
- 另有 `relock` 目标（A-30）

生成结果**不含**任何进程派生、`libc`、Python（DP9）。它的依赖面只有 Rust 工具链本身。

---

## 12. 测试策略与验收矩阵

### 12.1 布局（DP7）

| 层 | 位置 | 用途 |
| --- | --- | --- |
| 单元 | 各 crate `#[cfg(test)] mod tests` | 纯函数、类型不变量、解析/归一化 |
| 契约 | 各 crate `tests/` | 黑盒；`api` 走 `tower::ServiceExt::oneshot`，`storage` 走真实临时 SQLite |
| 编排 | `app/tests/` | **进程内**跑完整 `run(ProcessEnv, StopStream)`：注入合成环境与合成停止请求，用 `testkit::LogCapture` 断言事件顺序 |
| 夹具 | `testkit` | 日志捕获、临时安装根、临时库。**不含 `ProcessEnv` 构造器**——它住在 `app`，邻接表禁止 `testkit → app`，构造器留在 `app/tests/mod common`（§18.2 C-16） |

**没有"真实二进制 + 真实信号"这一层**（DP9）。生成结果不引入 `libc`、不引入 Python、不 `spawn` 自己。
代价是三条边界事实失去自动化证据，见 §12.4；换来的是所有编排断言变成确定性的（不依赖调度与信号投递时机）。

`--test-threads=1` 不需要（§5.8 的零全局态是前提）。

### 12.1.1 使编排可进程内验证的结构前提

不测进程只有在**进程边界被收敛成一个可注入的类型**时才成立。两个收敛点：

```rust
// app/src/boot/env.rs —— 全仓库唯一读取进程环境的地方
pub struct ProcessEnv {
    pub exe_path:           PathBuf,                       // std::env::current_exe()
    pub args:               Vec<OsString>,                 // std::env::args_os()
    pub vars:               BTreeMap<OsString, OsString>,  // std::env::vars_os()
    pub stderr_is_terminal: bool,                          // IsTerminal
}
impl ProcessEnv { pub fn capture() -> io::Result<Self>; }   // 无分支、无决策

// app/src/lifecycle/stop.rs —— 纯状态机，不碰 tokio::signal
pub struct StopPolicy { /* 第一次 / 第二次 / 第三次的语义 */ }
impl StopPolicy { pub fn on_request(&mut self, now: Instant) -> StopAction; }
//                 StopAction = Begin(Plan) | Escalate(Plan) | ForceExit

// app/src/signals.rs —— 全仓库唯一调用 tokio::signal 的地方，只做转发
pub fn os_stop_stream() -> impl Stream<Item = StopRequest>;
```

`run(env: ProcessEnv, stops: impl Stream<Item = StopRequest>) -> RunReport` 是整个进程的可测入口；
`main()` 缩成三行：`capture()` → `run()` → `exit_code(&report)`。

这带来一个**比进程测试更强**的结论：`std::env::current_dir` 在整个 workspace 里**一次都不出现**，由 D23 的门禁逐字节证明——比"换三个 cwd 启动结果一致"覆盖面更大（后者只证明了被试的那三个）。

### 12.2 不变量 → 证据形态 → 已知限制

| 不变量 | 证据形态 | 已知限制 |
| --- | --- | --- |
| 依赖边等于邻接表 | `structure` 相等比较 | 只看 `cargo metadata` 的构建图，不看 `#[path]` 之类的源码级引用 |
| 不留 detached task | supervisor drop 后 `Arc` 存活计数归零 | 证明的是句柄被 abort，不证明 OS 线程已回收 |
| 不留同 key 双化身 | 注册表唯一性返回 `Result` + 重入用例 | — |
| 关停总时长有上界 | `spawn_blocking` 造不可中断任务，断言在 `d2` 放弃并如实上报 | 上界是**逻辑时钟**上的；真实机器负载下的偏差不在覆盖面 |
| 内层预算严格不超外层 | 对一组预算组合做属性式遍历 | 组合是枚举的，不是穷尽的 |
| 第二次停止加速而不续命 | `StopPolicy` 纯状态机 + 注入时钟；再由编排层用合成停止流跑一遍双请求场景 | 不覆盖"两个真实信号在 μs 级同时到达"的竞态 |
| 存储最后关 | 编排层用例经 `LogCapture` 断言 `storage_close_result` 晚于全部 `task_exit` | 依赖日志顺序 = 事件顺序（由 D19 与单线程写入保证） |
| 冷段漏分类会红 | **编译失败**（穷尽解构） | 编译失败不是一条"测试"，无法在 `acceptance.md` 里记一个测试名；记为"编译期证据 + 一条负向编译用例（`trybuild` 形态）" |
| watch 恒等于生效值 | 冷段整批拒绝 + 热段 `send_replace` 原子换值 + 生成号 | 半热档的"下次面重启生效"只有报告证据，没有跨重启证据 |
| 重载失败保留 last-good | 坏文件 / 缺文件 / 坏权限三条编排层场景（真实临时目录，注入 `ProcessEnv`） | — |
| 路径锚点唯一 | ①`std::env::current_dir` 全仓库零出现（门禁逐字节证明）；②`install_root_from(exe)` 纯函数用例；③编排层用三份不同 `ProcessEnv.exe_path` 断言配置解析随锚点走 | 不覆盖 symlink 农场；`current_exe()` 自身的返回值无证据（见 §12.4） |
| 门面不泄漏 sqlx | `core` 不依赖 sqlx（编译期）+ 公共出口清单比对 | 清单比对是文本级的 |
| 二进制只有一个后端 | 已解析 feature 集断言 | 断言的是 feature，不是符号表 |
| 迁移新增文件会触发重编译 | 增 / 改 / 删各一次 `fresh` 标志检查 | — |
| 错误信封覆盖路由补集 | 打 `/`、`/nope`、`/v2/x` | — |
| 状态码不拍平 | 四条用例 + 两两不等断言 | — |
| release 下 panic 可观察 | 真实 release 产物 + 逐行 stdout 精确比对 + `panic="abort"` 负向探针 | 只在模板仓库的门禁里跑，不进生成结果的 `make check`（构建一次 release 太贵） |
| 两套装配互不干扰 | 两端口 / 两库文件 / 两 root token 的进程内用例 | — |
| 非 TTY 输出不含 ANSI | 注入 `stderr_is_terminal: false/true` 各一次，写入内存 buffer 断言 `\x1b` 的有无 | **两个分支都有证据**（这是注入相对进程管道测试的净收益）；剩下的缺口只是 `IsTerminal` 判断本身，属 std 行为 |
| 日志格式是接口 | 合成事件的逐字段快照 | 快照测试对字段**顺序**敏感，改动要同步改快照 |
| 任意合法名字都能生成出自身门禁全绿的项目 | 名字矩阵四组 + `verify` + `verify-git` | 四组是选出来的边界，不是全集 |

### 12.3 零 skip

任何"声明了却连不上"的前提一律**红而不跳**（A-20/A-21/A-22 的直接反面）。测试代码里不出现 `return` 式的跳过分支；前提不满足就 `panic!` 并说明缺什么。

### 12.4 明确没有自动化证据的三条边界事实（DP9 的代价）

删掉进程测试层之后，以下三条**必须进「无自动化证据」册**，不得用任何测试伪装成已验证。册子分两处落：`docs/acceptance.md` 开头点名（表格形态——写成条目会被 `acceptance-ids` 当成验收 ID 去找同名用例，而这几条按定义没有用例），理由、残余风险与每次验证有没有手工证据写在 `docs/verification.md` 第 4 节。

| 事实 | 为什么无法在进程内证明 | 残余风险有多大 |
| --- | --- | --- |
| `tokio::signal` 真的把 SIGTERM / SIGINT / SIGHUP 送进 `os_stop_stream()` | 需要向自己发信号，即需要 `libc`/`nix` 或外部进程 | `signals.rs` 是**纯转发**：无分支、无状态、无预算计算。所有语义都在 `StopPolicy` 里且有用例。一旦这层写错，`cargo run` 第一次 Ctrl-C 就会暴露 |
| `std::env::current_exe()` 在真实安装布局下返回可执行文件本身 | `ProcessEnv::capture()` 只能在真实进程里取值 | `capture()` 无分支无决策；所有**判断**都在 `install_root_from(exe)` 里且有用例 |
| 进程退出码被操作系统观察到 | 需要父进程 | `exit_code(&RunReport)` 是纯函数且有用例；`main()` 只做 `ExitCode::from` |

三条的共同形态：**把不可测的部分压缩成无分支的适配器，把全部判断挪进可测的纯函数**。这是"删掉进程测试"这个决定能被接受的唯一前提——如果适配器里含判断，本节就不成立。该前提由 **§2.9 的 D57 / D58** 强制，二者是本裁决的配套硬约束而非可选项。

> 本节之外，`docs/acceptance.md` 还需标注一条无自动化证据项：`StorageError::VersionConflict`（保留以满足 §7.1 的归一化契约，但模板不预建业务表，因而无真实构造路径 —— 见 §16 的 P2）。
> Phase 1.5 之后，原本同属此列的 `StorageError::Unavailable` / `Internal` **不再无证据**：`InMemoryStorage`（D66）可直接注入这两类故障，用来验证上层的降级行为。这是 D66 除"举证门面可替换"之外的第二项收益。

---

## 13. 决策点裁决记录

### ⭐ DP1 — Rust 源是否经 Liquid 渲染

| 方案 | 代价 |
| --- | --- |
| **A**：`.rs` 含占位符 + 黑名单收窄渲染面 | ①rustfmt 名字依赖必须事后补救：`fmt-portable` 静态扫描 + `fmt-matrix` 换名重跑 + `template-sync.py`，合计 2 个 Python 脚本 + 2 条门禁；②含 `{{` 的 Rust 格式串必须逐个进 `exclude`，名单随文件增减手工维护（A 当前 36 个 `.rs` 含占位符，`exclude` 只有 1 条）；③锁文件反向映射只能做文本替换，`GEN_PREFIX=tokio` 会改坏第三方包名（A-31）；④渲染面默认全开，新增文件默认进入渲染面 |
| **B**：`include` 白名单 + 根 manifest `package =` 别名，`.rs` 逐字节复制 | ①项目名进不了 Rust 源，需要另一条路把"服务名"送进运行期——`env!("CARGO_BIN_NAME")` 解决（零成本，编译期常量）；②`[[bin]] name = "{{crate_name}}"` 使得集成测试里拿不到字面量 bin 名（`CARGO_BIN_EXE_<name>` 要求字面量）——**这项代价随 DP9 一起消失**：不做进程测试就无人需要 bin 路径；③白名单写窄了会让该渲染的文件被逐字节拷贝（B-26） |

**选择：B**。
**判据**：A 的三件套修的是一个**由渲染面本身制造出来的问题**。删掉渲染面，问题连同它的补救一起消失，净减 2 个脚本 + 2 条门禁 + 1 份手工名单。B 的三项代价里，①有零成本解法，③是一行 glob 修正（`**/Cargo.toml`），只有②留下一个真实取舍（见 §16 待拍板 P1）。
F2 的"沿用 A 的形态"在此处指：树外 `make gen`、五目标 Makefile 骨架、`cargo generate --test` 自检、`lock` 回写——这四条全部保留（§11.1）。

### ⭐ DP2 — release `panic` 策略

| 方案 | 代价 |
| --- | --- |
| `abort` | 体积略小、栈展开确定性；但 ①`debug_assert!` 类检查在 release 全部消失（A-01）；②§5.2 要求的"区分 panic 与被外部 abort"在 release **不可能**成立；③`CatchPanicLayer` 失效，一个 handler panic 杀掉整个进程（A 的 `tower-http` 甚至没开 `catch-panic` feature——X-03） |
| `unwind` | 二进制略大、每个 `Result` 边界带展开表；换来 supervisor 的退出分类在 release 真实成立、handler panic 答成统一信封 |

**选择：`unwind`**。
**判据**：§5.2 的退出分类是本设计的硬主题之一。选 `abort` 等于承认这条纪律只在 debug 下成立——那它就不是纪律。证据形态必须是**产物级**的（真实 release 构建 + 逐行 stdout 比对 + `cfg(panic)` 编译守卫的负向探针），不是读 manifest 断言（B 的 `check_release.py` 是现成形态）。

**落点与实测**（`scripts/release-probe.sh`，已落地，4/4 绿）。门禁把一个 example 注入生成树（**不**随模板交付：它是门禁的一部分，不是模板的一部分），以 release 构建后直接执行产物，逐行相等比对六行：

```
name=ticker        kind=panicked        message=release probe: deliberate panic
terminal=true      failure=true         debug_assertions=false
```

四条断言各自能红，理由分别是：

- `release-artifact` —— 最后一行由**产物自己**回答"这是不是 release"，脚本说了不算；脚本正是可能把 `--release` 传丢的那一方。实测其判别力：同一个探针在 dev profile 下前五行**一字不差**，只有这一行翻成 `true`。换句话说，退出分类在两种 profile 下都成立，而这个门禁的全部价值就在于它证明的是 **release 下**也成立。
- `panic-classified` —— 相等比对而非"含有"：多打出来的那一行，可能正是某个分类走错之后新长出来的。
- `process-survives` —— 退出码必须是 0。`abort` 之下这里会是 134（SIGABRT），而且那六行根本打不出来。另查 stderr 里有没有默认 panic hook 的痕迹：分类取到了消息、人读的那一份却没有，是一种只在事故当天才会发现的静默。
- `abort-guard` —— 改的是根清单的 `[profile.release]`，不是 `RUSTFLAGS`。两条路都能打开 `cfg(panic = "abort")`，但前者是真的会发生的那一种（有人为了"减小体积"顺手写上去）；后者验的是 cargo 怎么把 flag 转成 cfg，那是 cargo 的行为，不是本模板的纪律。断言不只要求"构建失败"，还要求失败文本含 `this workspace requires`——一个碰巧失败的构建不能当成守卫生效的证据。实测报错正是 `core/src/task/exit.rs` 的那条 `compile_error!`。

两趟构建都带 `--target <host-triple>`（`rustc -vV` 自报，不写死）。不带的话 cargo 会用同一份 profile 去编宿主工具，而 proc macro 必须以 unwind 编译，于是负向探针会红在一个与被测纪律无关的地方、报错里完全不提 `compile_error!`。见 C-149。

### ⭐ DP3 — 启动协议

| 方案 | 代价 |
| --- | --- |
| **注册即运行**（A） | 简单；但顺序不变量只能用基数代理断言（A-09），面的 future `Output = ()` 让"起不来"与"正常收尾"同形（A-14），端口绑定被当成就绪 |
| **prepared ≠ committed**（B） | 多一层状态机 + 每个面一个 `oneshot` 回执；换来"绑定 ≠ 就绪"可被契约测试钉死、启动失败携带首因、`/readyz` 有明确判据 |

**选择：显式启动提交门**。
**判据**：`/readyz` 若没有提交门就只能拿"端口能连上"当就绪，而端口能连上恰恰是**最早**发生的事。B 的 `listener_binding_does_not_open_the_startup_commit_gate` 是这条纪律唯一有说服力的证据形态。
两处修正：提交条件用显式谓词而非 `select!` 分支顺序（修 B-37）；提交后任何配置再读都要重新进"校验 → 提交"（修 B-38）。

### DP4 — crate 集合

| 方案 | 代价 |
| --- | --- |
| 保留 `reconcile` | 约 1800 行无消费者代码随 workspace 一起被测；且它重复实现取消/收敛语义并与 supervisor 不一致（A-24..A-27） |
| 删 `reconcile` | 期望集/生命周期骨架不再预铺，用户要自己写；README 的垂直切片步骤承担指导 |
| 保留 `testkit` | 多一个 workspace 成员进邻接表 |
| 不要 `testkit` | 日志捕获的 callsite 缓存规避要在 4 个 crate 里各复制一份 |

**选择：6 crate —— 删 `reconcile`，留 `testkit`**（理由表见 §3.1）。
**判据**：预铺的判据是"有没有消费者"。`reconcile` 没有；`testkit` 有（D32、D24 等一组日志断言）。

### DP5 — 依赖版本钉法

| 方案 | 代价 |
| --- | --- |
| 全精确 `=x.y.z`（B） | 安全补丁不会自动进来，必须改 manifest；且 B 没有任何审计门禁知道钉着的版本已有 CVE（B-29）。与随发布的 `Cargo.lock` 叠加是**双重锁定** |
| semver 区间 + 随发布 `Cargo.lock`（A 的钉法，但 A 的注释在说谎——X-02） | 复现性由锁文件提供；用户 `cargo update` 能拿到补丁；代价是不同时间生成的项目若跑了 `cargo update` 会得到不同补丁版本 |

**选择：semver 区间 + 随发布 `Cargo.lock` + 每条依赖一句"为什么"**。
**判据**：复现性的载体是锁文件，不是 manifest 的写法；`=x.y.z` 在有锁文件的前提下只多做了一件事——挡住补丁。同时定三条纪律：①注释不得声称 manifest 里没有的钉法（修 X-02）；②模板仓库 CI 定期 `cargo audit`（补 B-29）；③README 的"改 version"步骤必须紧跟 `make relock`（修 A-30）。

### DP6 — 工具链钉法

| 方案 | 代价 |
| --- | --- |
| 只声明 MSRV（A） | 格式类门禁不可复现：CI `rustup update stable` 与开发机可能是两个 rustfmt（X-05） |
| `rust-toolchain.toml` 钉死（B） | 模板仓库可复现；但若把它也交给生成结果，每个新项目被永久钉在一个会老化的版本上 |

**选择：模板仓库钉 `rust-toolchain.toml`；生成结果只声明 `rust-version`，不带工具链文件**。
**判据**：需要字节级可复现的是**模板自己的门禁**（`structure` 做逐字节比较、`fmt` 结果参与结论），不是用户的项目。生成结果带工具链文件等于替用户做了一个会过期的决定。
配套：`resolver = "3"`（edition 2024 的 MSRV 感知解析；修 X-04 的 `resolver = "2"` 与 `rust-version` 互相抵消）；`rust-version` 声明的是**我们实测过的那一个版本**，README 明写这不是兼容下限承诺（§11 禁止声称未验证的平台）。

### DP7 — 测试布局

| 方案 | 代价 |
| --- | --- |
| 全部 `tests/`（A） | 只能测公共 API；A 因此出现代理断言（A-09 基数代理、A-16 指错来源）与静默跳过（A-20/A-22） |
| 全部同 crate（B） | 私有缝好测，但黑盒契约容易变成白盒；B 用 Python 补真实进程层 |

**选择：分层混合**（§12.1）：单元同 crate、契约 `tests/`、编排 `app/tests/`（进程内跑完整 `run()`）、夹具 `testkit`。
**判据**：判据不是"放哪儿"，而是"这条断言能不能发现它声称守护的违例"。据此：顺序不变量用顺序证据（不用基数）、黑盒断言指向真实来源、零 skip。
与 DP9 联动：没有进程层，"编排"就必须是一个**可调用的函数**而不是 `main()` 里的一段——这是 §12.1.1 两个收敛点的由来。

### DP8 — 冷段判别机制

| 方案 | 代价 |
| --- | --- |
| 后缀通判 + 冷前缀白名单（A） | 未登记默认判"热" → 报告写 `applied` 而消费方仍用旧值，比明说"要重启"更糟（A 自己的注释这么写的）；且回滚是**第二份**人工清单（X-08） |
| 类型全等 + 整批拒绝（B） | 拒绝是安全的；但报告路径清单仍是测试里手写的 10 项（B-17） |

**选择：B 的类型全等 + 整批拒绝，并把"归档"提升为编译期义务**（§6.2）。
**判据**：§5.5 明确禁止"按面登记"，要求给出**漏登记就会红的机制**。无 `..` 的穷尽解构让新增字段编译不过——这是唯一不依赖任何人记得写测试的机制。报告清单由派生产生，修掉 B-17。

### DP9 — 真实进程测试是否进门禁

| 方案 | 代价 |
| --- | --- |
| 全量照搬 B（250 场景 Python） | 给每个新项目引入 Python ≥3.11 依赖；与 F5 的单一入口冲突（B 把它拆到 `check-process` 且无人触发——X-09/B-31）；人工用例清单（B-11）、接受两种退出码（B-12） |
| 取子集，用 Rust 重写 | 不引入 Python，但要给生成结果加 `libc` dev-dependency 和一段 `unsafe` 信号调用；且真实信号 + 真实进程的时序引入不确定性，`--test-threads` 与端口/临时目录的竞争都要自己处理 |
| **完全不进程测试**：把进程边界收敛成可注入的适配器，编排层在进程内跑完整 `run()` | 三条边界事实失去自动化证据（§12.4）；`cargo fmt`/`clippy`/`test` 三条门禁之外不再有第四条 |

**选择：完全不做进程测试**。生成结果既不引入 Python，也不引入 `libc`，也不 `spawn` 自己。
**判据**：三点。
① **F5 的原文是"生成结果的 `make check` 是门禁的唯一定义"，而 §9 给 `Makefile.project` 定的就是三条门禁**——四条门禁版本本身是我对交付物规格的偏离，去掉进程层反而回到规格。
② 进程测试的价值不在"起了个进程"，而在**它断言的那些不变量**。把 `ProcessEnv` 和 `StopPolicy` 抽出来之后（§12.1.1），原计划 6 个场景里有 5 个在进程内能测得**更彻底**：cwd 独立性从"试了三个 cwd"升级成"`current_dir` 全仓库零出现"；ANSI 从只能测非 TTY 一侧升级成两侧都测；双信号加速从依赖真实时序升级成注入时钟的确定性用例。
③ 真正失去的只有"适配器本身是否接对了线"，而适配器被 D57 约束成无分支转发——**这是唯一能让这个决定成立的前提**，所以 D57 是本裁决的配套硬约束，不是可选项。

进生成结果 `make check` 的编排层场景（`app/tests/`，全部进程内）：
1. 三份不同 `ProcessEnv.exe_path` → 配置与数据目录随锚点走，且 `install_root` 事件一致（D23）
2. 配置来源优先级三档各一条（`--config` / 环境变量 / 缺省）（修 B-10 的默认档零覆盖）
3. 合成停止请求 → 完整关停序列：`shutdown_started` 先于任何 cancel、`storage_close_result` 晚于全部 `task_exit`、`unreaped_tasks=0`、`RunReport::succeeded()`、`exit_code(&report) == 0`
4. 双停止请求加速（D18）+ 三次强退
5. 坏配置启动 → 结构化事件、不回显取值、`exit_code` 非 0
6. `stderr_is_terminal` 两个取值各一次 → ANSI 有/无（D31）

留在模板仓库门禁的（不进生成结果）：release panic 产物探针（DP2 要求产物级证据，它构建并运行一个 release example，不需要信号）、名字矩阵、`--git` 生成。

### DP10 — 存储启动的可取消性与关闭所有者

| 方案 | 代价 |
| --- | --- |
| 启动不可取消、storage 自己关池 | 慢盘/坏路径下启动挂住且无人监管（A-19）；关闭时机与面的收尾无法排序 |
| 可取消 + 关闭权归 app | 多一个 `StorageOwner` 类型和一个握手 |

**选择：`StorageOwner` 持池归 app，`Arc<dyn Storage>` 交上层；启动全程受 `cancel` 约束；关池超时不替换原始失败**（§7.2）。
**判据**：§5.4 要求"存储最后关"，这就要求关闭动作在**关停序列**里而不是在某个析构里；而"最后"只有在 app 持池时才有意义。`CloseOutcome` 三态 + 原因，不设 `NotCreated`。

### DP11 — HTTP 就绪语义

| 方案 | 代价 |
| --- | --- |
| `ready` = 端口可连 | 最早发生的事被当成就绪；draining 期间仍 ready |
| `ready` = `Phase == Running` 且存储健康，且探测有界 | 多一次存储往返 |

**选择：后者**（§8.2、§8.5）。
**判据**：探测超时由 `handler_timeout` 覆盖 → 有界；跨越 draining 的探测**一律不 ready**，否则负载均衡会在关停窗口继续送流量，把 §5.4 的全部功夫抵消掉。

### DP12 — 提取器包装是否预铺

| 方案 | 代价 |
| --- | --- |
| 不预铺 | 第一个带请求体的 handler 上洞无声打开——A 自己在 `api/README.md` 里写明"没有一行代码改动，也没有一条测试会红"（A-15） |
| 预铺但只写 README 禁令（A 的实际状态） | 等于没预铺：无 `clippy.toml`，`disallowed_types` 无从配置 |
| 预铺 + lint 强制 | 多一个 `clippy.toml` 和四条用例 |

**选择：预铺 + `clippy.toml` 的 `disallowed-types` 强制 + 状态码不拍平**（§8.4）。
**判据**：这补的不是新功能，是**已写下的响应契约上的洞**——契约已经说"所有对外错误同形"，缺的是让它成立的那一层。与"不预铺"不冲突：`extract` 有消费者（契约本身），而 A-18 的四个存储语义变体没有。
边界：**不**预铺 `From<StorageError> for HttpError`（只能产出 500 的零信息仪式），改用无 `_` 臂的 `match` 让新增变体编译不过；`VersionConflict` 显式标注无自动化证据。

### DP13 — 横切能力准入

逐项裁决见 §9。摘要：**内置 7 项**（结构化日志 / TraceLayer / CatchPanicLayer / handler 超时 / body 上限 / 系统端点 / SIGHUP 重载），**不进 7 项**（CORS / OpenAPI / Prometheus / HTTP 客户端 / 文件日志 / reload 端点 / 认证鉴权），**可选 0 项**。
**判据**：三条。①是不是部署事实（F6）→ 不进；②在模板里有没有消费者（A-18）→ 没有则不进；③不进的每一项必须在 `README.project.md` 的「当前边界」列名并说明在哪加——B-20 的教训是"文档宣称了没有的能力"，反过来"有意不做却不说"同样是缺陷。

---

## 14. 风险台账与取舍记录

| # | 风险 | 触发条件 | 缓解 | 残留 |
| --- | --- | --- | --- | --- |
| R1 | ~~`.rs` 不渲染 → 进程测试拿不到字面量 bin 名~~ | ~~DP1 选 B~~ | **已消解**：DP9 定为不做进程测试，无人需要 bin 路径；`[[bin]] name = "{{crate_name}}"` 保留（二进制叫 `myproj`），且**不需要 `build.rs`** | 无 |
| R1′ | 适配器（`boot/env.rs`、`signals.rs`）接错线时无自动化证据 | DP9 | D57 把两个文件约束成无分支转发；`cargo run` 第一次 Ctrl-C 即暴露 | 三条边界事实明列于 §12.4，验收文档标「无自动化证据」 |
| R2 | `unwind` 下 `CatchPanicLayer` 仍**不能**捕获流式 body 中途的 panic | 响应体流式推送 | 在 `api` 的模块文档里明写覆盖边界（B 的做法） | 这一类 panic 仍会杀死连接；不宣称覆盖 |
| R3 | 关停上界是逻辑时钟上的 | 机器过载 | 预算留余量；报告如实写 unreaped | 真实机器上的偏差不在覆盖面 |
| R4 | `spawn_blocking` 一旦开始不可中断 | 用户在面里用了它 | 配置读取改用异步 IO（修 B-36）；README 明写这条 tokio 语义 | 用户自己写的 blocking 工作仍不可中断，只能报成 unreaped |
| R5 | 半热档只有报告证据，没有跨重启证据 | 半热字段变更 | 报告列入"下次面重启生效" | 明写为已知限制（§12.2） |
| R6 | TTY 分支无端到端证据 | 交互式终端 | 只断言非 TTY 侧 | 明写"没有真 PTY 覆盖"（B-20 的反面教训） |
| R7 | 生成结果的 `cargo fmt --check` 随用户 rustfmt 版本浮动 | DP6 选"生成结果不带工具链文件" | README 明写 | 用户升级 rustfmt 可能在未改代码时见红 |
| R8 | `install_root` 在开发期是 `target/debug/` | §5.9 的纪律 | `make dev-config` 一键准备 + README 明写 | 第一次 `cargo run` 的直觉冲突 |
| R9 | `structure` 的公共出口清单比对是文本级的 | D4 | 清单与 `pub use` 两侧集合相等 | 不检查类型签名 |
| R10 | 名字矩阵四组是选出来的边界 | D49 | 覆盖连字符 / 极长 / 极短 / 包名冲突 | 不是全集；正则之外的名字被 pre hook 拒绝 |
| R11 | 删掉 `reconcile` 后，"怎么写一个有状态的后台面"失去示范 | DP4 | `worker/ticker` 作为最小样板 + README 垂直切片步骤 | 复杂编排要用户自己设计 |
| R12 | `semver` 区间 + 锁文件下，不同时间 `cargo update` 得到不同补丁 | DP5 | 锁文件随模板发布；CI 定期 `cargo audit` | 用户主动 update 后的组合不在模板覆盖面 |

---

## 15. 证据索引

### 15.1 本次已在本机核实

| 事实 | 出处 |
| --- | --- |
| `cargo-generate 0.24.0` 的 `--test` 展开 `$CWD`，可用 `CARGO_GENERATE_TEST_CMD` 覆盖测试命令，且 implies `--verbose` | 本机 `cargo generate --help` 输出 |
| A 的 `.rs` 中 36 个含 `{{`，B 为 0 | 两个 worktree 的 grep |
| B 用 `env!("CARGO_BIN_NAME")` 把 bin 名送进运行期，`.rs` 里没有任何项目名字面量 | B `app/src/main.rs`、`app/src/config/load.rs` |
| B 的 `include` 白名单为 `["Cargo.toml", "Cargo.lock", "*/Cargo.toml", "README.project.md"]`（一层 glob） | B `cargo-generate.toml` |
| B 的冷段判别是 `BootConfig: PartialEq` 全等 + `changed_fields` 穷尽解构 | B `app/src/config/mod.rs` |
| B 的 `rust-toolchain.toml` 钉 `1.97.1` / `minimal` / `rustfmt`+`clippy` | B `rust-toolchain.toml` |
| A 无 `rust-toolchain.toml`，CI 两处 `rustup update stable` | A `.github/workflows/ci.yml` |
| A 的 `panic = "abort"`，`tower-http` 未开 `catch-panic` | A `Cargo.toml` |
| 两分支均无 `cors` / `utoipa` / `prometheus` / `metrics-exporter` / `reqwest` / `tracing-appender` 依赖 | 两份根 `Cargo.toml` |

### 15.2 尚未核实、Phase 2 必须自证（现为待证假设，不得当作结论使用）

> **状态（Phase 2 收尾）**：下表**全部实测结案**，逐条结论与探针见 **§18**。最后一项 `--git` 时序随 `scripts/verify-git.sh` 落地时结案。
> 其中两条**推翻了设计原文**（sqlx 的 `bundled` 归属、tower-http 超时的空 body），已就地修订 D39 / §7.5 / §8.5。

| 待证项 | 为什么必须自证 | 结论 |
| --- | --- | --- |
| `tower-http` 所选版本是否有 `TimeoutLayer::with_status_code` | A 用了它并声称超时回 408；本机 registry 无该版本源码（audit §8.2） | **已证，且改判**（§18 P-D）：方法存在；`new` 已弃用；但其响应体为空、绕过信封 ⇒ 不用它 |
| ~~`cargo:rustc-env` 是否对集成测试 target 生效~~ | **已不需要核实**：DP9 定案后没有 `app/build.rs`，也没有需要 bin 路径的测试 | — |
| `--git` 模式下 `.genignore` / `ignore` / `hooks/` 自删的时序 | 两分支门禁均不走 `--git`，无证据（§6 坑 3，audit §8.2） | **已证成立**：`verify-git.sh::absent-via-git` 在克隆出来的树上跑完整条 `structure.sh`，其中 `absent` 一项把 `ignore` 名单与 `post.rhai::assert_absent` 交叉比对后再核对生成树。`ignore` 的删除、hook 文件自删、`hooks/` 空目录收尾三件事在 `--git` 下与 `--path` 下结果相同 |
| ~~bin 路径推导在 `--target` / 自定义 `CARGO_TARGET_DIR` 下是否成立~~ | **已不需要核实**：同上 | — |
| `app` 同时有 lib + bin 时，`env!("CARGO_BIN_NAME")` 在 bin target 内取到的是 `[[bin]].name` 而非包名 | 配置环境变量前缀（§6.3）完全依赖这一条 | **已证成立**，并附带一条新约束：lib 侧读不到（§18 P-C） |
| tokio 所选版本 `Interval::reset_at` 只改下一次 deadline、`JoinSet::spawn_on` 的跨 runtime 归属 | §5.2/§5.3 直接依赖这两条语义；B 的 `runtime_semantics.rs` 是现成的探针形态，但要对**本设计钉的版本**重跑 | **已证成立**（§18 P-B） |
| sqlx 所选版本 `sqlite-bundled` 的准确 feature 名与闭包边界 | D39 的断言字面量 | **已证，且改判**（§18 P-A/P-E）：`bundled` 属 `libsqlite3-sys`；`migrate!` 另需 `macros` |
| `resolver = "3"` 所需的最低 cargo 版本 | DP6 | **已证可用**（cargo 1.95.0 实建通过，§18 P-F） |

`docs/references.md` 在 Phase 2 随版本钉定一并产出：每条外部语义写清**版本号 + 出处**，并注明是"读了官方文档"还是"写了探针实测"。本阶段不预写——预写就是声称未验证的事实。

---

## 16. 闸门 1 裁决记录

> 闸门 1 已通过。以下六项是提交审批时列出的待拍板项与人工裁决结果，保留原文以便追溯"为什么是这样"。

| # | 议题 | 裁决 | 落点 |
| --- | --- | --- | --- |
| P1 | `.rs` 不渲染后，进程测试如何拿到二进制路径 | 认可原处理（保留 `[[bin]] name = "{{crate_name}}"`，二进制叫 `myproj`）。**随后因 P6 自动消解**：不做进程测试 ⇒ 无人需要 bin 路径 ⇒ **不加 `app/build.rs`**。等于用零机制拿到了"好看的二进制名" | §13 DP1、§14 R1 |
| P2 | `VersionConflict` 变体去留（§5.7 要求 vs §11「不预铺」） | 认可原处理：变体保留以满足 §5.7 的归一化契约；HTTP 映射由**无 `_` 臂**的 `match` 覆盖（新增变体编译不过）；`docs/acceptance.md` 对它显式标「无自动化证据」，不写伪装成验证的测试 | §7.1、§8.3、§13 DP12 |
| P3 | 生成结果是否带 `rust-toolchain.toml` | 认可原处理：**不带**。生成结果只声明 `rust-version`；残留代价 R7（用户升级 rustfmt 可能在未改代码时见红）写入风险台账并在 README 明示 | §13 DP6、§14 R7 |
| P4 | 审计台账写了行号，与 §2「不写行号」不一致 | 认可原处理：台账**保留行号**（被审计的是冻结的备份分支，行号不会烂，且可核查性显著更高）；本设计文档继续只写文件路径 | `docs/audit-0913-0915.md` |
| P5 | 删 `reconcile` 是删掉整个 crate，不止"改一条边" | 理由被认可，**维持删除**。crate 集合定为 6 个 | §3.1、§13 DP4 |
| P6 | 生成结果的进程测试引入 `libc` dev-dependency | **否决，并进一步要求删除进程测试层，且不引入 Python**。DP9 据此重裁：把进程边界收敛成可注入的 `ProcessEnv` / `StopPolicy` / `os_stop_stream`，编排层在进程内跑完整 `run()`；新增 D57、D58 作为配套硬约束；三条边界事实明列为「无自动化证据」 | §12.1.1、§12.4、§2.9、§13 DP9 |

### P6 带来的连锁修改（全部已落入本文）

1. `app` 变成 **lib + bin 两个 target**，`main()` 缩成三行，`run(ProcessEnv, StopStream)` 成为可测入口（§3.4、D58）。
2. 生成结果的 `make check` 回到**三条门禁** `fmt / lint / test`——这正是 §9 对 `Makefile.project` 的原始规格，四条门禁版本本是我的偏离（§11.4）。
3. 生成结果的依赖面收敛为**只有 Rust 工具链**：无 Python、无 `libc`、无自我派生。
4. 五项原进程场景的证据强度**上升**（cwd 独立性、ANSI 双分支、双信号加速、关停顺序、配置来源优先级），一项（真实退出码）下降为无证据，详见 §12.2 / §12.4。
5. 新增 D57（适配器内不得有判断）——这是"删掉进程测试"能够成立的**唯一前提**，不是可选项。

---

## 17. Phase 1.5 — barter-rs 借鉴结论

完整研究见 [`docs/study-barter-rs.md`](study-barter-rs.md)（对象 commit `9770b27`，6 crate / 256 `.rs` / 约 40.7k 行）。本节登记它对本文造成的**实际差异**。

### 17.1 本文因此发生的修订

| # | 修订 | 章节 | 来源 |
| --- | --- | --- | --- |
| 1 | 装配拆 `assemble()`（纯构造零 `spawn`）/ `launch()` 两段 | §5.2、D59 | S1 |
| 2 | `Handle::current()` 只允许出现在装配入口一处 | D60 | S2 |
| 3 | `ExitKind::is_terminal()`，无 `_` 臂；"发生了什么"与"该怎么办"解耦 | §5.3、D61 | S3 |
| 4 | 残留任务物化为 `Vec<UnreapedTask>`（含 `stalled_at`）进 `RunReport` | §5.4 | S4 |
| 5 | `ShutdownClass::{Graceful, Abortable}` 注册时必填，只有前者分配 harvest 预算 | §5.4、D62 | S5 |
| 6 | 根 `[workspace.lints]` + `unused_crate_dependencies`；禁 crate 级 `allow` | D63 | S6 |
| 7 | `#[from]` 只允许用于本 workspace 错误类型 | D64 | S7 |
| 8 | 带 smart constructor 的 newtype 禁 `#[derive(Deserialize)]` | D65 | S8 |
| 9 | feature 门控的 `InMemoryStorage`，作为 §7.5 唯一的**肯定式**举证 | §7.5、D66、§11.4 | S9 |
| 10 | D1 的邻接表比较改在**已解析 feature 集**下做 | D1 | §4 末段 |

纪律 58 → 66。

### 17.2 两个方向性收获

**印证**：barter-rs 的引擎核心是纯同步 `Processor<Event>::process() -> Audit`，外套四个约 40 行的薄 runner 适配器。这与本文为 P6 引入的 `StopPolicy` + 无分支适配器是同一手法——D57/D58 因此从"我的推导"升级为"有 40k 行生产代码的外部先例"。

**反证**：它的关停路径全程**无超时预算**（一个卡住的组件让进程永不退出）、`JoinError` 被压成字符串（丢失 panic/cancel 区分）、`abort()` 后不回收不记录。这三条恰好是本文投入篇幅最多的地方（§5.4、§5.3、D16/D17），它以真实代码的形式给出了不这么做的后果。

### 17.3 明确不采纳

整数 arena 索引（需要启动期冻结的封闭实体集）、unbounded channel 无背压（公网服务下是 OOM 不是背压）、proc-macro crate（N<10 不值）、深度泛型参数化、`#[async_trait]`、三条 crate 级 `allow`、浮动 `rust-toolchain.toml`、clippy 不带 `--all-targets`。逐条理由见 `docs/study-barter-rs.md` §4。

---

## 18. Phase 2 实测修订登记

### 18.1 开工首日探针

> 本小节记录 **Phase 2 开工首日**对 §15.2 待证假设的逐条探针，以及由此产生的设计修订。
> 探针工程在树外（`/tmp/p2probe`，一次性），结论以「命令 + 实际输出」为准，不以记忆或类比为准。
> 之所以在写第一行产品代码之前先做这一批：**断言字面量写错的门禁比没有门禁更坏**——它恒为真且看起来是绿的。P-A 正是这一类。

| 探针 | 结论 | 对设计的影响 |
| --- | --- | --- |
| **P-A** sqlx 0.9.0 feature 闭包 | `--no-default-features` + `sqlite-bundled,runtime-tokio,migrate` 解析后：`sqlx = {_rt-tokio, _sqlite, migrate, runtime-tokio, sqlite-bundled, sqlx-sqlite}`；依赖图只含 `sqlx/sqlx-core/sqlx-sqlite/libsqlite3-sys`，**无 postgres/mysql/any** | ⚠️**推翻原文**：`bundled` 不是 sqlx 的 feature，而是 `libsqlite3-sys` 的。D39 与 §7.5 的断言改为**逐包**断言 |
| **P-E** `sqlx::migrate!` 可用性 | 不开 `macros` 时 `sqlx::migrate` **不存在**（E0433 实测）；加 `macros` 后可用，`migrator len = 1` | ⚠️**新增依赖事实**：`macros` 是 D37 的硬前提，非可选。代价实测 +6 个 normal 依赖包（122 → 128，含 `sqlx-macros(-core)`、`dotenvy`）。已并入 D39 断言 |
| **P-C** `env!("CARGO_BIN_NAME")` | bin target 内 = `[[bin]].name`（`probe-binary-name`），**不是**包名（`probe-pkg`）；lib 侧 `option_env!` = `None` | 原假设成立；**附带新约束**：前缀只能由 `main.rs` 读出并向下传参，写入 §6.3 |
| **P-D** tower-http 0.7.1 超时 | `with_status_code(StatusCode, Duration)` 存在；`new` 自 0.6.7 起 `#[deprecated]`，其默认值确为 `REQUEST_TIMEOUT`（A 的 408 说法属实）；但超时响应体是 `Response::new(B::default())`——**空 body、无 content-type** | ⚠️**推翻原文**：它绕过 §8.3 的统一信封，且 `new` 在 `-D warnings`（D63）下直接红。改为 `api` 自实现 `handler_timeout`，状态码取 504，`tower-http` 不开 `timeout` feature |
| **P-B** tokio 1.53.1 运行期语义 | `JoinSet::spawn_on` 跨 runtime 返回 `Ok("ran-on-other")`；`reset_at` 后首次 tick 在 53 ms（即重设点）、次次 tick 在 552 ms（= 重设点 + 周期） | 两条假设均成立：`reset_at` 只移动**下一次** deadline，周期自新点续算。§5.2/§5.3 不改 |
| **P-F** `resolver = "3"` | cargo 1.95.0 下 workspace 实建通过 | DP6 成立。`rust-version` 按「我们真正测过的那一版」钉为 1.95 |

**探针出处**：`tower-http` 的两条语义读的是本机 registry 源码 `~/.cargo/registry/src/index.crates.io-*/tower-http-0.7.1/src/timeout/service.rs`（弃用属性、默认状态码、空 body 构造三处均在该文件），其余为运行实测。全部将转写进 `docs/references.md`，各带版本号与「文档 / 探针」标注。

**这批修订的共同教训**：A 与 B 两个分支里凡是"看起来对"的断言，有两条实际是假的（P-A 的 feature 归属、P-D 的信封覆盖）。它们都属于**门禁自身写错**——不是漏写门禁，而是写了一条永远通过的门禁。§2 取证纪律要求「第三方语义带版本 + 出处」，其强制力就体现在这里。

### 18.2 落地期修订

> 本小节随层推进追加。凡是**代码落地时发现设计文本与现实不符**的地方都记在这里，
> 而不是默默按代码改掉文本——设计与实现分叉的位置本身就是要交付的信息。
> 分三类：⚠️**推翻原文**（文本是错的）、➕**补全原文**（文本没写到）、🔧**工程链待办**（本层发现、下一步落地）。

| # | 类别 | 内容 | 落点 |
| --- | --- | --- | --- |
| C-01 | ⚠️ | `TaskExit.runtime` 实为 `Option<RuntimeId>`，不是 §5.3 代码块里的裸 `RuntimeId`。理由与 D9 同源：元数据查不到时，`name` 和 `runtime` 一起缺失，而"查不到标识"与"怎么退的"必须正交。写成裸 `RuntimeId` 就得在查不到时编一个假的 runtime 标识 | `core/src/task/exit.rs`；§5.3 代码块已改 |
| C-02 | ➕ | `CloseOutcome` 三态与 §5.7 一致，但"到点未关"的归属原文没写：它折进 `Failed`，由 `CloseOutcome::timed_out()` 构造，措辞只此一处。判据是**从调用方看，"到点没关上"和"关时报错"是同一个结论——没有证据说明池已关上** | `core/src/storage.rs`；`timed_out_is_a_failure_with_a_stable_wording` |
| C-03 | ➕ | `ConfigPublisher` / `ConfigReader` 落在 `core::config`，不是装配层的私有类型。这是 §3.3 邻接表逼出来的：`worker` 和 `api` 都要持读端，而它们都只能依赖 `core`。写端非 `Clone` 因此成为唯一的写入端纪律载体 | `core/src/config/publish.rs` |
| C-04 | ➕ | 反序列化错误怎么进 `ConfigError` 而 `core` 不依赖 `serde_path_to_error`：`app` 拿到路径与消息两个字符串，自己构造 `ConfigError::Deserialize { path, message }`。`core` 只定义错误形态，不定义它从哪种解析器来 | `core/src/config/mod.rs` 模块文档 |
| C-05 | ➕ | 新增根 `clippy.toml`（`allow-unwrap-in-tests` / `allow-expect-in-tests` / `allow-panic-in-tests`）。它**不违反** D45/D63：D63 禁的是 crate 级 `#![allow(...)]`（给自己发豁免且没人知道），D45 的门禁扫的正是 `#![allow(` / `#![warn(` 的密度。而这三条是 lint **配置**，作用域由 `#[cfg(test)]` 结构性决定，不需要在任何 `.rs` 里写一个 `allow`。替代方案是给每个 `mod tests` 挂 `#[allow]`，那会散出几十条——恰好是 D45 要拦的形态 | 根 `clippy.toml` |
| C-06 | ➕ | `Config::clamp()` 返回 `Vec<ClampRecord>` 且**不打日志**。`core` 打一条、`app` 再打一条，会让运维以为钳了两次。启动路径由 `app` 逐条 `warn!`，重载路径并进重载报告 | `core/src/config/pipeline.rs` |
| C-07 | ➕ | §6.4 之外新增 `Accepted`：`evaluate_reload` 产出它，`ConfigPublisher::publish` 只收它，它没有公共构造函数。于是"冷字段变了不能热重载"是**类型事实**而非每个调用点各自遵守的规矩。这是 barter-rs 的 build/init 两段式（D59）在配置面的同形应用 | `core/src/config/{heat,publish}.rs` |
| C-08 | ➕ | `Secret<T>` / `SecretSource` **零调用点**——唯一后端是 SQLite，不带凭据。与 §5.7「不预铺没有消费者的东西」的张力已在模块文档里正面回答：①它是 D33 那条安全契约（可能承载敏感内容的字段必须是 `Secret`）的**执行机构**，没有它那条契约无从遵守；②形状由 D28 指定，不是猜的。同时明确**不引** `zeroize`——在一个会把配置整体 `Clone` 进 watch 通道的进程里，它给不出承诺的保证 | `core/src/config/secret.rs` 模块文档 |
| C-09 | ⚠️ | `Path::file_name()` 会把尾分隔符归一化掉：`Path::new("data/").file_name() == Some("data")`。原先按"`data/` 开不出文件"来写 `validate` 的理由是错的。`None` 的**全部**情形是：空路径、根（`/`）、以 `..` 结尾。`data/` 能不能开出文件取决于 `data` 当下是不是目录，那是文件系统事实，`core` 按 D23 拿不到也不该猜——留给 `storage` 打开连接时带着真实 io 原因失败 | `core/src/config/pipeline.rs`；`validate_rejects_only_what_cannot_possibly_run` 的 4 条用例 |
| C-10 | ⚠️ | `reap` 的回归用例必须跑在 `flavor = "multi_thread"` 上。它用一个同步阻塞的任务制造"abort 不生效"，而在默认的 current-thread 调度器上那次阻塞占住唯一工作线程，连 `reap` 自己都排不上队；等它睡完任务已正常退出，`unreaped` 为空——**用例测了个反面且显示为绿**。这与 §18.1 的共同教训同类：写了一条永远通过的断言 | `core/src/task/supervisor.rs::reap_gives_up_at_its_own_deadline` |
| C-11 | ⚠️ | `[workspace.lints.rust]` 里 `rust_2018_idioms` 是 lint **组**不是单条 lint。组与单条同在默认优先级 0 时 cargo 直接判错（`lint group ... has the same priority`），整个 workspace 编不过。必须写成 `{ level = "warn", priority = -1 }` | 根 `Cargo.toml` |
| C-12 | ➕ | `core` **不依赖** `tokio-util`。根 `CancellationToken` 建在 `app`，child token 由各面自己持有；`core` 的任务面只描述"谁在跑、怎么退的"，不持有取消句柄。清单里留一条没人 `use` 的依赖会被 `unused_crate_dependencies`（D63）指出来——那条 lint 正是为此存在 | `core/Cargo.toml` |
| C-13 | 🔧 | `cargo-generate.toml` 必须把 `**/*.rs` 排除出渲染面。`core/src/config/expand.rs` 的 `#[error]` 格式串里有 `${{` / `{{NAME}}`（Rust 的 `{` 转义），Liquid 会把它们当模板标记。这不是额外代价——DP1 选 B 路线本就保证项目名不出现在任何 `.rs` 里，排除 `.rs` 只是把那条保证写进渲染配置 | 工程链步骤，`cargo-generate.toml` |
| C-14 | 🔧 | D4 的公共出口清单比对**不能用 Python**（P6）。已在本机验证不变量成立（`core` 两侧集合相等），比对本身改写成 shell（`sed` 抽 `pub use` 与顶层 `pub` 项 → `sort` → `comm`）留到工程链步骤 | 工程链步骤，`scripts/` |
| C-15 | ⚠️ | §3.4 写的是「全局 subscriber + **线程局部** sink」，后半句是错的。要断言的事件（`task_exit` / `storage_close_result`）由 tokio 的工作线程发出，线程局部 sink 看不见它们——按原文写出来的顺序断言会**永远通过**，与 C-10、§18.1 的共同教训同类。改成**全局 sink**，代价是捕获期互斥（做日志断言的用例之间串行），`LogCapture::start` 用条件变量等前一个 drop。另两条随之确定：①互斥不能用长期持有的 `MutexGuard`，那会让 `LogCapture` 变成 `!Send` 并在 `async` 用例里踩 `clippy::await_holding_lock`（我们设成 `deny`）；②捕获期间别的并行用例的事件也会落进同一个 sink，所以断言一律按事件名走，不断言"总共几条" | `testkit/src/log.rs` 模块文档；回归用例 `events_from_other_threads_are_captured`（线程局部实现过不了它） |
| C-16 | ⚠️ | §12.1 的夹具一行把「`ProcessEnv` 构造器」列在 `testkit`，与 §3.3 邻接表第 `testkit` 行冲突——`ProcessEnv` 住在 `app`，而 `testkit → app` 是 ✗。构造器留在 `app/tests/` 自己的 `mod common` 里：那里只有一个消费者，也只该有一个。邻接表不为一个夹具让路，否则"测试基础设施变成第七层"就从这一行开始 | `testkit/src/lib.rs` 的「这里刻意没有什么」；§12.1 那一行已划掉 `ProcessEnv` |
| C-17 | ➕ | 要被断言的事件**必须显式写 `name:`**——`tracing::info!(name: "shutdown_started", …)`。不写的话 tracing 生成的名字是 `event <文件>:<行号>`，会随行号漂移，拿它做断言等于把测试绑在行号上（与取证纪律"不写行号"同源）。语法在 tracing 0.1.44 上已实测可编译，`Event::metadata().name()` 返回的就是那个字面量。这条约束对 `worker` / `api` / `app` 每一条可断言事件都成立，不只是 `testkit` | `testkit/src/log.rs`；§5.6 事件名契约的执行前提 |
| C-18 | ➕ | `cfg(test)` 对 `tests/` 下的**集成测试目标同样成立**（已实测），于是 `clippy.toml` 的 `allow-*-in-tests`（C-05）覆盖契约层与编排层的 `.rs`，不需要额外配置。但它**不覆盖夹具的库代码**——`testkit` 自己的非测试代码不在豁免内，所以夹具 API 一律返回 `io::Result`，由调用方去 `.expect()`。这反而是想要的形状：夹具坏了要能被区分于被测代码坏了 | `testkit/src/fixtures.rs` 全部构造器；`testkit/README.md`「它刻意不做的事」末行 |
| C-19 | 🔧 | 「`testkit` 进不了二进制」目前只是一条**声明**，没有机械保证。工程链要加一条门禁：扫所有成员的 `[dependencies]`（不含 `[dev-dependencies]`），出现 `-testkit` 即判红。这条比邻接表门禁更窄也更关键——邻接表管方向，它管**依赖的种类**，而把 dev 依赖误写成常规依赖不会被任何编译器报出来 | 工程链步骤，`scripts/`；`Makefile.project` 的 `structure` 门禁 |
| C-20 | ⚠️ | §7.2 的「诊断路径（取根因的二次连接）继承同一 deadline（修 A-19）」**整条取消**——那个危险类别在 sqlx 0.9 上结构性地不存在。实测：`SqlitePool::connect_with` 不是惰性的，它当场取一条连接，失败时直接把真实原因返回；没有"先给个 `PoolTimedOut`、再另开一条连接去问为什么"这一步，也就没有第二条连接可以漏掉 deadline。A-19 修的是它自己引入的问题 | §7.2 该条已划掉；`storage/src/open.rs` |
| C-21 | ➕ | WAL 边车（`-wal` / `-shm`）**创建时继承主库文件的模式**（实测），所以 0600 纪律只需要在主库文件上落一次，不需要逐个 chmod。原文只说了"库文件 0600"，没回答边车——而边车里有完整的未 checkpoint 数据，漏掉它等于 0600 白做 | `storage/src/open.rs::the_database_and_both_sidecars_are_owner_only` |
| C-22 | ➕ | `open` 还负责**建父目录**，§7 没写这一步。理由是首次部署的真实路径：安装根下的 `data/` 在第一次启动前不存在。放在 `core::paths` 不行——那一层按 D23 不碰文件系统 | `storage/src/open.rs::prepare_file` |
| C-23 | ➕ | 库文件由 `open` **自己以 0600 创建**，连接选项写死 `create_if_missing(false)`。让驱动去建的话文件会拿到 umask 给的模式（通常 0644），0600 就被静默绕过了——而且绕过之后没有任何一条断言会红，因为文件确实存在、库确实能用 | `storage/src/sqlite.rs::base_options` |
| C-24 | ➕ | `storage` **不依赖 `tracing`**，一条事件都不发。理由与 C-06 同源：两层都记，同一次失败在日志里出现两次、措辞还不一样。代价是 `Migration { stage, hint }` 带不出"第几号迁移"，于是 `hint` 的写法被固定成**"去哪儿看"**（`migrations/` 目录、`_sqlx_migrations` 表），而不是"出了什么事"——这条由一条用例守着，不是纪律 | `storage/Cargo.toml` 的依赖缺席说明；`every_migration_stage_is_reachable_and_carries_a_hint` |
| C-25 | ➕ | `StorageError` **不加** `Cancelled` 变体。启动期被取消走 `Internal { context: "open was cancelled during <step>" }`。判据：这个错误只有一个消费者（`app` 的启动路径），而那条路径上"被取消"与"起不来"要做的事完全一样——都是放弃启动、按已取消的流程收尾。为一个不改变任何决策的区分去加一个公共变体，等于让所有上层都要多匹配一条永远走同一分支的臂。`context` 里带**停在哪一步**，排障要的信息一点没少 | `storage/src/open.rs::cancelled_at` |
| C-26 | ➕ | sqlx-sqlite **不实现** `DatabaseError::constraint()`（那是 Postgres 独有的），约束名只能从消息里按 `"constraint failed: "` 前缀解析。另一条同源的：`DatabaseError::code()` 返回的是**扩展**结果码（如 `2067`），要 `& 0xFF` 才得到主码（BUSY=5 / LOCKED=6 / READONLY=8）。两条都是第三方语义，直接决定 `error_map` 的写法 | `storage/src/error_map.rs::{constraint_of,primary_code}` |
| C-27 | ➕ | `sqlx::Error` / `ErrorKind` / `MigrateError` 全是 `#[non_exhaustive]`，而 `MigrateError` 里还有一个 `#[deprecated]` 变体——显式匹配它会因 `-D warnings` 直接判红。于是两个归一化函数都**必须**留兜底臂，且兜底的措辞写明"未分类"，让日志能把"我们认得但没细分"和"sqlx 升级后冒出来的新东西"区分开 | `storage/src/error_map.rs` 两处兜底臂的注释 |
| C-28 | ⚠️ | **DP1 的落地少一件东西**：`[workspace.dependencies]` 的 `package =` 别名只管**跨 crate**引用；`tests/` 下的集成测试目标按 **lib 目标名**链接被测 crate，拿不到别名。不补的话集成用例只能写展开后的真名，项目名就漏进了 `.rs`。补法是每个成员清单写 `[lib] name = "service_*"`。**"让它 dev-depend 自己"这条路实测走不通**——cargo 识别出自依赖后会丢掉 `package =` 重命名，仍按真名注入 | 根 `Cargo.toml` 第 1 条纪律；`{core,storage,testkit}/Cargo.toml` 的 `[lib]` |
| C-29 | ➕ | `unused_crate_dependencies`（D63）在**集成测试目标**上按目标逐个判：`tests/x.rs` 没用到本 crate 的某条普通依赖就是一条警告，而本工作区 `-D warnings`。契约层恰好**刻意**不碰 `sqlx`，于是要写一行 `use sqlx as _;`。这不削弱那条断言——`as _` 不引入任何可命名的东西——反而让"哪个测试文件刻意没碰驱动"变成了源码里看得见的声明 | `storage/tests/facade.rs` 头部 |
| C-30 | ➕ | 全模板 **doctest 计数为 0**，示例代码一律标 ` ```text ` 围栏。理由：doctest 里的 `use` 必须写 crate 真名，而真名由模板变量展开。标 ` ```ignore ` **不够**——它仍然被计成一条 doctest（只是不跑），`cargo test --doc` 会报 `1 ignored`。原先 `testkit` 的用法示例就是这种形态，已改 | `testkit/src/log.rs::LogCapture` 文档；`storage/README.md` 末节 |
| C-31 | 🔧 | 两条门禁写法在这一层定型，留给工程链：①`--workspace` 之下 cargo **不接受裸 feature 名**，D66 那趟必须写成 `--features {{crate_prefix}}-storage/test-utils`（`Makefile.project` 参与渲染，可以写模板变量）；②D4 的清单比对对**有 feature 的 crate 要分两份比**，不能取并集——取并集的话，把一个本该门控的名字漏进默认出口就查不出来，而那正是 F3 要守住的线。③零 doctest 是可机械检查的不变量：`cargo test --doc` 的计数必须是 0/0/0 | 工程链步骤，`Makefile.project` / `scripts/` |
| C-32 | ⚠️ | §7.4 的「`MAX_MIGRATION_VERSION` 查询带 `WHERE success = TRUE` 过滤」**整条取消**：模板里没有这个查询的消费者——§8.2 的 `/v1/info` 给的是构建信息，门面上也没有读 schema 版本的方法。预铺一条没人调用的查询正是 §7.5 自己禁的事（也是任务书 §11 的"不预铺猜出来的形状"）。那条过滤的**理由**（失败的迁移在 `_sqlx_migrations` 里也有行，不过滤就会报出一个从未成功应用过的版本号）搬进 `storage/migrations/README.md`，留给第一个真需要它的人——知识保留，形状不预铺 | §7.4 该条已划掉；`storage/migrations/README.md` |
| C-33 | ⚠️ | §5.2 第 3 点「`Ack` 携带面特有的凭证：HTTP 面回传**真实绑定的 `SocketAddr`**（端口为 0 时这是唯一的知情途径）」**整条取消**——括号里那句话在同一份设计里已被另一条决定推翻：§3.4 把 `bind(addr) -> TcpListener` 单列成 `api` 的公共出口，`app` 在 `launch()` **之前**就同步 bind 完，真实地址从 `TcpListener` 上直接读得到。回执因此是**纯信号**，不带任何负载。副作用是想要的：一次性启动通道带了负载，迟早有人往里塞第二样东西 | `core/src/task/ack.rs` 模块文档；`core/README.md`「几个不显眼但重要的形状」首条 |
| C-34 | ⚠️ | §5.2 的伪码写成 `spawn_on(handle, plane_future, ack_tx)`，而实际签名是 `spawn_on(&mut self, spec, handle, future)`——**没有 ack 参数**。于是 `AckSender` 只能被 `build()` **捕获进面的 future 里**，而这要求它的类型对 `worker` 和 `api` 都可见。两者都在 `app` 之下、互不可依赖，唯一的共同下层是 `core`——`ack_channel` 因此落在 `core::task`，与退出侧的 `TaskExit` 共用同一套面标识。另有一条随之确定：面名写在**接收端**，由装配层填，面拿不到写名字的机会，"任务是 A、回执记成 B"在类型上就不成立（与 `RuntimeId::of(handle)` 同一手法） | `core/src/task/ack.rs`；`worker/src/ticker.rs::TickerPlane::build` |
| C-35 | ⚠️ | **`abort_boot` 不发 `Draining`**（§5.4：它只做"取消 → 短宽限收割 → 关存储"），阶段停在 `Starting` 上。于是"等提交"这一步**必须**把 `cancel` 与 `wait_for_running()` 放进同一个 `biased select!`；只 `await` 阶段的写法会在启动失败路径上一直挂着，最后靠 abort 收掉——一次干净的启动失败在退出报告里变成 `Cancelled`，看起来像一次超时。这条对**每一个**面都成立，不只是 `ticker` | `worker/src/ticker.rs::run`；用例 `a_boot_that_never_publishes_running_still_lets_the_plane_return_cleanly` |
| C-36 | ➕ | §3.4 给 `worker` 列的出口只有 `TickerPlane::build`，落地时**加了 `TickerPlane::SPEC`**。理由是 `TaskSpec` 自己的文档那句话：「"这个面需不需要优雅收尾"只有写这个面的人知道」——让 `app` 在登记时替它猜一个，等于把那句话作废。代价也写在明处：使用者往 `tick` 里填了带缓冲的写入之后，`SPEC` 那一行要跟着改成 `Graceful`，而改它的人就被迫重新想一遍关停预算 | §3.4 `worker` 行已补；`worker/src/ticker.rs::TickerPlane::SPEC`；用例 `the_spec_says_abortable_and_the_plane_owns_that_answer` |
| C-37 | ➕ | `worker` 的普通依赖 `tokio` **不开 `rt`**——本层一个任务都不 spawn，少给一个 feature 比写一条纪律可靠（"能 spawn 就会有人 spawn"）。而 dev-dependency 的 `tokio` 要 `rt` 而**不是** `rt-multi-thread`：本层用例全跑在暂停的时钟上，`tokio::time::pause()` 只在 current-thread 调度器里成立，多线程调度器下直接 panic（实测）。默认间隔 30 秒起步，不暂停时钟就只能靠真实等待，门禁会变慢且"慢到底算不算过"随机器负载漂 | `worker/Cargo.toml` 两处依赖注释；`worker/tests/ticker.rs` 头部 |
| C-38 | ➕ | **C-29 的补全**：`unused_crate_dependencies` 不只在集成测试目标上逐个判，它对 **lib 的 test 目标**同样成立——dev-dependency 会被链进那个目标。`worker` 没有 `#[cfg(test)] mod tests`，于是 `service-testkit` 在那里成了"写了却没人 use"。写法是 `#[cfg(test)] use service_testkit as _;`。另一条路（编几个用不上的单元用例把夹具用起来）等于为了一条 lint 往库里塞假断言 | `worker/src/lib.rs` 末行；对照 `storage/tests/facade.rs` 的 `use sqlx as _;` |
| C-39 | ⚠️ | **C-18 前半句要收窄。** `allow-*-in-tests` 是**结构性**豁免，clippy 只认两种位置：`#[cfg(test)]` 模块内，或 `#[test]` 函数体内。集成测试文件里的**辅助函数**两样都不占——`tests/ticker.rs` 的 `Rig::republish_interval` 里一句 `panic!` 在 `-D warnings` 下直接判红（实测）。修法不是加 `#[allow]`，是让夹具**返回值**、把断言交回 `#[test]`：失败位置因此指向用例而不是夹具，本来也更对。这条对后续每一层的 `tests/` 都成立 | `worker/tests/ticker.rs::Rig::republish_interval`；`core/README.md` 测试节已补注意事项 |
| C-40 | ➕ | `worker` **依赖 `tracing`**，与 C-24 的 `storage` 正好相反。不是双标：存储的失败能通过 `StorageError` 上抛到 `app` 的唯一调用点去记，tick **没有**这样的上抛点——不在这里记就在任何地方都记不到。级别定 `info` 而不是 `debug`，因为这是模板里唯一一个"改了配置立刻能观察到效果"的行为，`docs/verification.md` 靠它举证热重载；一个默认看不见的样板举不了任何证。事件里 `generation` 字段最关键：「reload 报了成功、面还在用旧值」靠它直接看得出来，不必从间隔去推断 | `worker/Cargo.toml` 的 `tracing` 注释；`worker/src/ticker.rs::tick` |
| C-41 | ➕ | 循环的形状定为**先读后睡**，于是热重载的生效延迟上界是**一个旧间隔**（已睡下的那一轮早把旧值取走了，重载不叫醒它）。换来的是：`load()` 之外没有任何可失败的步骤落在提交门之后——B-38 的病灶正是"回执之后用新配置重建 timer"。同源的第二条：本层**不**做防御性再钳位，`tick_interval` 的合法区间只有三段式管线一份答案（§6.3 启动与重载同管线），再钳一次就等于让两份答案迟早不一致 | `worker/src/ticker.rs` 模块文档两节；用例 `a_reloaded_interval_takes_effect_after_the_round_that_already_read_the_old_one` 逐拍核对 `generation` 与 `slept_ms` |
| C-42 | ➕ | `worker.enabled`（半热）**由 `app` 在装配期读**，不在面里读。面里读会得到一个登记了、有 `TaskSpec`、会进关停报告、却什么都不做的空面——那是最难排查的一种"看起来在跑"。另记一条工具语义：`tick` 保留 `async` 签名而当前实现零 `await`，不能用 `#[expect(clippy::unused_async)]` 压——`unused_async` 属 pedantic，本工作区只启用点名的 lint，于是那条 expectation 永远不被满足，`unfulfilled_lint_expectations` 反而在 `-D warnings` 下判红。改成散文说明理由 | `worker/src/ticker.rs` 模块文档「`enabled` 不在这一层读」与 `tick` 的文档节 |

| C-43 | ⚠️ | 根清单给 `tower-http` 的 `limit` feature 写的理由（"`DefaultBodyLimit` 的底座"）**是错的**，feature 已删。实测 `DefaultBodyLimit` 是 axum-core 0.5.6 自己的东西：靠 `RequestExt::into_limited_body()` 把 body 包进 `http_body_util::Limited`（`DEFAULT_LIMIT = 2_097_152`），超限的 `LengthLimitError` 被 downcast 成 413，整条路径与 tower-http 无关。`unused_crate_dependencies` 只看 crate、看不见多余的 feature，所以这类错误**只能靠人删**——§11「不为了好看加依赖 / feature」针对的正是这种"理由写得头头是道但事实不成立"的形态 | 根 `Cargo.toml`；`api/Cargo.toml` 的 `tower-http` 注释 |
| C-44 | ⚠️ | **`CatchPanicLayer::new()` 不能用**，与 §18.1 的 P-D 是**同一类**缺陷。它的默认响应工厂 `DefaultResponseForPanic` 回的是 500 + `text/plain; charset=utf-8` 的 `"Service panicked"`——又一个绕过 §8.3 统一信封的第三方层。改用 `CatchPanicLayer::custom(respond_to_panic)`；`ResponseForPanic` 对 `FnMut(Box<dyn Any + Send + 'static>) -> Response<B> + Clone` 有一条泛型实现，所以一个普通 `fn` 项就够，不必自定义类型。由此提炼出一条可复用判据，已写进 `api/src/lib.rs`：**凡是能自己产出响应的第三方层，都要先问它产出的是不是信封**。P-D 是上一版发现的，这一条**两个备份分支都没发现** | `api/src/middleware/panic.rs` 模块文档；`api/src/router.rs` 的 `.layer(CatchPanicLayer::custom(...))` |
| C-45 | ⚠️ | D46 的落点写作 `api/src/config.rs`，而 `api` **没有** `config.rs` 也不该有——它不读配置（§3.3：配置从 `AppState` 的读端来）。`handler_timeout` 实际住在 `core/src/config/types.rs::HttpConfig`，与 `bind_addr`、`body_limit_bytes` 同一节。落点已改 | §9 D46 行落点已划掉；`core/src/config/types.rs` |
| C-46 | ⚠️ | §8.4 那张状态码表**不在 `api` 里重抄**，包装提取器直接委托 axum rejection 的 `.status()`。除"少一份会漂移的副本"外，关键理由是**axum 的分档比那张表细**：`FailedToDeserializePathParams` 是手写的（不是 `__composite_rejection!` 宏生成的），它把「路径参数类型不符」判 400、把「handler 的 `Path<T>` 元数与路由参数个数对不上」判 **500**——后者是程序员写错了，不是客户端发错了。手抄表必然把两者压平成同一个 400，于是一个自己写错的 bug 会被报成客户端错误，在监控上永远查不出来。另记：`__composite_rejection!` 生成的枚举都是 `#[non_exhaustive]` 且 `status()`/`body_text()` 是委托实现，包装层只能**转发**，不能手写 `match` | §8.4 已补一行 500 与一段说明；`api/src/extract.rs`；用例 `a_path_extractor_misuse_is_a_500_and_withholds_its_detail` |
| C-47 | ⚠️ | §8.3 的「`nest` 内 fallback + **顶层** fallback」两处都要，**收缩成只要顶层一处**。实测：内层路由器**没有**自己的 fallback 时会继承外层的，所以内层那条是多余的；真正危险的是反方向——**给 `nest` 进去的路由器加了 fallback，顶层那条就对整个子树失效**，那一片的 404 退回 axum 的空 body，而没有任何编译期或启动期信号会提醒你。原文把安全方向和危险方向写反了。模板里因此一条 `nest` 都没有，`/v1/info` 是一条普通路由 | §8.3 首条已划掉；`api/README.md`「两个会静悄悄出事的地方」 |
| C-48 | ➕ | §8.3 的漏点清单漏了**第六个**：路径存在但方法不对时 axum 回 405 + **空 body**。由 `method_not_allowed_fallback` 接管。随之定死一条装配顺序纪律：**先路由、再 fallback、再 405、最后 layer**——`method_not_allowed_fallback` 只改**已经注册**的 `MethodRouter`（axum 文档原话 "all previously registered"），而 `Router::layer` 能覆盖 `fallback_router` 与 `catch_all_fallback` 的前提也是它们已经挂上。把 `.route()` 挪到 `.layer()` 之后，新路由会静悄悄地不带任何中间件，**不报错、不告警** | `api/src/router.rs` 装配注释；用例 `every_error_path_renders_the_same_three_key_envelope` 含 `POST /healthz` 一档 |
| C-49 | ⚠️ | §8.2 给 `/readyz` 列的三项原因枚举（`starting` / `draining` / `storage_unavailable`）**扩张成** `Phase::as_str()` 全集 ∪ `storage_unavailable`。理由与 C-24 同源：生命周期阶段的名字只有一份，这里不另起一套同义词；用 `as_str()` 相当于自动覆盖全部六个阶段，少一处要手工维护的枚举。配套一条单元用例断言 `storage_unavailable` 与**任何**阶段名都不相等——否则客户端分不清"探测失败"和"处在某个阶段" | §8.2 该行已划掉；`api/src/system.rs::{not_serving, the_readiness_reasons_never_collide_with_a_phase_name}` |
| C-50 | ⚠️ | §3.4 给 `api` 列的出口三项与落地不符，逐项：①`bind` 返回 `std::io::Result<TcpListener>` 不是裸 `TcpListener`（它还顺带 `set_nonblocking(true)`，`tokio` 的 `from_std` 要求调用方保证这一点）；②`build` 的参数是 `(listener, router, ack, cancel)` 不是 `(listener, state, cancel)`——`Router` 而非 `AppState`，因为装配 router 是 `router()` 的事，面只负责跑；`ack` 是 C-34 的后果（`spawn_on` 没有 ack 参数，只能由 `build` 捕获）；③实际出口是 **9 项**，`HttpError` / `ENVELOPE_KEYS` / `Json` / `Path` / `Query` / `router` 原文一个都没列，而它们正是使用者天天要碰的那几个 | §3.4 该行已划掉；`api/src/lib.rs` 的 6 条 `pub use` |
| C-51 | ➕ | `HttpPlane::SPEC` 加在这一层，理由与 C-36 的 `TickerPlane::SPEC` **逐字相同**：「这个面需不需要优雅收尾」只有写这个面的人知道。取值 `ShutdownClass::Graceful`——与 `ticker` 的 `Abortable` 相反，因为 HTTP 面在排空期要把**在飞的请求**答完，那正是 graceful 预算存在的理由 | `api/src/serve.rs::HttpPlane::SPEC`；用例 `the_spec_says_graceful_and_the_plane_owns_that_answer` |
| C-52 | ⚠️ | §8.5「面在收到 `Phase::Running` 之前不处理业务」**改写**：这件事由 `/readyz` 在 `Running` 之前回 503 实现，**不**由推迟 accept 实现。推迟 accept 会让就绪探针**自己也连不上**，编排器读到的是「连接被拒」而不是「已绑定但未就绪」——后者恰恰是就绪探针存在的全部意义。原文那条用例名（B 的 `listener_binding_does_not_open_the_startup_commit_gate`）测的是另一件事：bind 不提前开提交门，那条仍然成立且由 `a_taken_port_fails_at_bind_time` 覆盖 | §8.5 该条已划掉；`api/src/system.rs::readyz` |
| C-53 | ⚠️ | §8.5 超时中间件形态里的 `HttpError::Timeout.into_response()` 暗示 `HttpError` 是**枚举**，实际是**结构体**（字段全私有，构造途径只有 `client` / `server` 两条），那行写作 `HttpError::timeout()`。枚举形态会把「哪个变体配哪个状态码」摊到公共 API 上，于是任何人都能构造一个 5xx 并塞进任意 `String`——而「5xx 的 message 必须是 `&'static str`」（§8.3）正是靠 `server(…, description: &'static str)` 与 `client(…, message: impl Into<Box<str>>)` 的**参数类型差异**保证的。这不是风格选择：枚举写法下那条安全契约没有执行机构 | §8.5 代码块已补注；`api/src/error.rs`「为什么不是枚举」 |
| C-54 | ➕ | `AppState` 多一个**私有**字段 `started: Instant`，因此只能经 `AppState::new(...)` 构造。§8.2 要求 `/v1/info` 报 uptime，而那个数字必须是**这个进程**的；让装配层用结构体字面量自己填一个，它就成了"装配时随手写的值"——与 A-16 同型（两处都"看起来是对的"，改了一处另一处不跟） | `api/src/state.rs::{new, uptime}` |
| C-55 | ➕ | 信封三个键名定为 `error` / `message` / `status`，且 `ENVELOPE_KEYS` 是**唯一**真值来源（连序列化兜底那串字面量也照它写）。配套一条反向纪律：模板自己的五条端点响应体**刻意避开**这三个名字（`live` / `ready` / `version` / `package` / `uptime_seconds`），于是 §8.3 那条「成功响应里信封三个键一个都不在」测的是真事，而不是一条恰好成立的断言 | `api/src/response.rs::ENVELOPE_KEYS`；`api/src/system.rs` 三个响应体的文档；用例 `success_bodies_never_use_an_envelope_key` |
| C-56 | ➕ | `api` 的 dev-dependency `tokio` 要加 `io-util`：面的生命周期用例**手写一行 HTTP/1.1** 发进真 socket。模板里没有 HTTP 客户端，也不为一条用例引一个（§11）。手写请求行反而更诚实——它证明的是"这个端口上真的有人在按 HTTP 应答"，与路由树的内部结构无关。同层另记：绝大多数契约用例走 `ServiceExt::oneshot` 不起端口，只有"`from_std` 注册到哪个 runtime"和"取消后还接不接连接"这两件事 `oneshot` 摸不到 | `api/Cargo.toml` dev-deps 注释；`api/tests/contract.rs::the_plane_acks_then_serves_then_returns_cleanly_on_cancel` |
| C-57 | ⚠️ | **C-17 要收紧，而且它被违反了。** C-17 写的是「**要被断言的**事件必须显式写 `name:`」——那个条件句是个漏洞：没人能预先知道将来哪条事件会被测试用到。落地时 `core/src/lifecycle.rs` 与 `api` 的 9 处事件全都把标识符塞进了**消息**位置、没写 `name:`，于是 `LogCapture::find` 找不到它们（它比对的是 `CapturedEvent::name`），两条契约用例红了才暴露——**在此之前没有任何门禁看得见这个缺陷**。改法不是改测试而是改源码：10 处全部补上显式 `name:`，消息位置留给给人读的那句话。条款收紧为「**每一个**事件都要显式 `name:`」。`worker/src/ticker.rs` 早已合规，`storage` 按 C-24 不发事件 | 新增 **§2.5 D67**；`core/src/lifecycle.rs` 内联注释；`api/src/lib.rs`「事件的名字和那句话是两件东西」；`api/README.md` 事件表（9 条） |
| C-58 | 🔧 | C-57 那个缺陷**对所有现有门禁都不可见**（能编译、clippy 干净、fmt 干净），所以工程链要加一条结构检查：扫所有 `tracing::{trace,debug,info,warn,error}!` 调用，第一个参数不是 `name:` 的判红。这条与 C-19（testkit 不进二进制）同类——管的都是"编译器永远不会告诉你的事" | 工程链步骤，`Makefile.project` 的 `structure` 门禁 |
| C-59 | ➕ | §5.5 的配置类型清单里**没有** `[runtime.extra.*]` 这一段，而 §4.2 又要求它存在——两节对不上。补的是 `RuntimeConfig.extra`，键即名字。`main` 同时是**保留名**：`[runtime.extra.main]` 被 `validate` 当场拒掉，否则关停的逆序表里会出现两个同名 runtime，而日志上分不出是哪一个 | `core/src/config/types.rs::{RuntimeConfig, RuntimeThreads}` |
| C-60 | ⚠️ | 取退出记录的方法叫 `next_exit`，不是设计文本写的 `next()`。这个类型上还有 `harvest` / `reap`，一个光叫 `next()` 的方法放进 `select!` 的一臂里读不出它产出的是什么——而那一臂正是提交门用来发现"面死在回执之前"的地方 | `core/src/task/supervisor.rs::next_exit` |
| C-61 | ⚠️ | `app` 的公共出口是 **8** 项，不是 §3.4 写的 4 项。多出来的是 `ProcessSignal` / `os_signal_stream`（信号类型必须能被用例构造，否则"塞一条合成信号流"无从写起）与 `StopCause` / `StartupFailure`（退出报告的两个可读字段）。签名也改了两处：`run` 收 `io::Result<ProcessEnv>`，`ProcessEnv::capture` 收 `bin_name` | `app/src/lib.rs` 的 `pub use` 块；`app/README.md`「公共出口」 |
| C-62 | ⚠️ | **D30 的顺序做不到原样。** 原文要求"启动失败以结构化事件出现"，但过滤器与格式就写在配置文件里——配置读取本身是唯一一个发生在遥测就绪**之前**的可失败步骤。落地形态：`run` 把读取的 `Result` 拿在手上，失败时用**默认**遥测设置临时装一个 subscriber 把它发出去（C-91），成功时由 `replay` 在 subscriber 就绪后补记。性质成立，顺序有调整 | `app/src/telemetry.rs` 模块文档；`app/src/lib.rs::{replay, fail_before_telemetry}` |
| C-63 | ➕ | D32（过滤器非法时回落并留痕）的落点从设计稿的装配入口移进 `telemetry.rs`：判定发生在解析过滤器的那一行，挪到别处就得把"要不要回落"这个结论跨模块传一次 | `app/src/telemetry.rs::FilterOutcome`；`an_invalid_filter_falls_back_and_says_so` |
| C-64 | 🔧 | **D60 的门禁判据要改。** 原文写"`app` 内 `Handle::current` 出现次数 `== 1`"，落地是 **0**——装配层收的是 `&Executors`，句柄由 `RuntimeSet::executors()` 显式给出，一次隐式读取都不需要。判据改成「`app` 的**非测试**代码里 `Handle::current()` 出现 0 次」，并显式豁免 `rt.rs` 里那一处 `Handle::try_current()`：它在 `Drop` 里**探测**"我是不是正被在 async 上下文里 drop"，不是取句柄来用 | `Makefile.project` 的 `structure` 门禁；`app/src/rt.rs` 的 `Drop` |
| C-65 | ➕ | `RuntimeShutdown` **不记 `timed_out`**，只记 `budget` 与 `elapsed`。`Runtime::shutdown_timeout` 什么都不返回，"超时了没"只能靠比时间去猜，而预算为零且任务已清空时这个猜法必然误报。与其给一个有时说谎的布尔，不如把两个真实数字交出去，让读日志的人自己比 | `app/src/rt.rs::RuntimeShutdown` |
| C-66 | ➕ | `assemble` 是 **async**，设计稿写的是同步。原因是打开存储要 `await`，而把它做成同步就得在装配路径上 `block_on` 一次——那等于在一个 runtime 里对同一个 runtime 阻塞。记为一次偏离而不是默默改：它让"装配可被取消"成为可能，也因此让 `boot::gate` 的取消臂有意义 | `app/src/boot/assembly.rs::assemble` |
| C-67 | ⚠️ | **D56 的门禁定义是错的。** 原文实际扫的是"非 ASCII 字符"，而 F4 明令文档与注释用中文——照原文执行的话，交付物里每一份 README 都是红的。判据重新定义为语义的那一半：「**随交付物发布的文件必须是写给生成项目的使用者看的**」，也就是不得出现模板仓库自身的路径、分支名、本设计稿的节号。这一条没法用字符集扫，只能靠一份显式的文件清单 + 关键词黑名单。门禁归属也跟着变：它验的是**生成出来的那棵树**，所以落在模板仓库的 `gen` 审计里，不在生成结果的 `Makefile.project` 里——生成结果没有义务知道模板长什么样 | `scripts/gen.sh` 的审计段；`docs/verification.md` |
| C-68 | ➕ | D24（不许静默）在 `config/bootstrap.rs` 上有一处**形态**例外：这个模块跑完之前 subscriber 还没装，所以它一行 `warn!` 都发不出去。留痕改成结构性的——回落与写模板作为 `TemplateAction`、钳位作为 `Loaded.clamps` 返回给 `run`，由它补记。这比 `warn!` 更好：一条日志只能靠抓日志断言，而这两个是可以直接 `assert_eq!` 的值 | `app/src/config/bootstrap.rs::{TemplateAction, Loaded}`；`app/src/lib.rs::replay` |
| C-69 | ⚠️ | §4.5 代价表原先指的证据用例名是设计期编的，落地后一条都对不上。现在**每一条有落点的行都指向一条真实用例**，且一个用例只扛一条断言：`default_config_declares_no_extra_runtime`（配置面）、`resolve_happens_once_at_startup`（解析面）、`single_runtime_shutdown_visits_exactly_one_runtime`（关停面）、`single_runtime_spawns_no_extra_threads`（线程面）。另外三行标「编译即证据」——那不是偷懒，是它们真的没有运行期形态可断言 | `core/src/config/pipeline.rs`；`app/src/rt.rs` 的 `tests` |
| C-70 | 🔧 | `toml` 要显式开 `serde` + `std` 两个 feature。缺省 feature 集不含它们，而缺了 `serde` 时 `toml::from_str` 根本不存在——这类"依赖装了但用不了"的错只在第一次真写调用时暴露 | 根 `Cargo.toml` 的 `[workspace.dependencies]` |
| C-71 | ⚠️ | `tracing-subscriber` 要显式开 `ansi` feature。不开的话 `with_ansi(true)` **在运行期 panic**，而不是编译期报错——也就是说，"开发机上跑 `cargo run` 直接崩"是唯一的暴露方式，CI 里 stderr 不是终端反而不会触发 | 根 `Cargo.toml`；`app/src/telemetry.rs` |
| C-72 | ⚠️ | `expand` 会扫**整个文件**，注释也不例外。于是内嵌配置模板里凡是给人看的 `$` 都必须转义成 `$$`，否则首次启动写出来的那份模板会在下一次读取时把注释里的示例当成变量引用去展开 | `core/src/config/expand.rs` 模块文档；`app/src/config/bootstrap.rs` 的内嵌模板；`embedded_template_round_trips_to_default` |
| C-73 | ⚠️ | **`TaskName::ConfigReload` 删除。** 设计稿把配置重载列成了第三个任务面，但落地后它没有 spawn 点——重载是编排循环里的一件事，不是一个常驻任务。一个登记了却没人 spawn 的面名正是 §11 的「预铺猜测的形状」，而且它会让关停报告里永远缺一条闭合记录。`TaskName` 现在**只有两个变体**，文件里写着不许猜着加第三个 | `core/src/task/name.rs` |
| C-74 | ⚠️ | **`signals.rs` 只实现 unix，非 unix 平台 `compile_error!`**，而不是给一个没验证过的实现（§11「不得声称未测平台」）。补齐很便宜：`run` 收的是 `impl Stream`，换一个产生器就行，编排层一行都不用动。这条要进 `README.project.md` 的「当前边界」 | `app/src/signals.rs` 的 `#[cfg(not(unix))]` 分支 |
| C-75 | ⚠️ | `api` 的公共出口是 **10** 项不是 9：新增 `business_routes`。业务路由必须能被单独拿出来，否则"业务 404 走信封、系统端点不走"这条边界就只能靠读 `router()` 的实现去确认 | `api/src/lib.rs::business_routes` |
| C-76 | ⚠️ | `StopCause` 有**四**个变体，不只是"信号"。另外三个是：某个面退出了、面全没了、信号流断了。只有第一个算"被要求停下"——这正是 `succeeded()` 六项合取里第①项的判据，把它们并成一个变体，"进程不是被要求停的，是自己死的"就没地方表达了 | `app/src/lifecycle/stop.rs::StopCause`；`only_a_signal_counts_as_an_expected_stop` |
| C-77 | ⚠️ | **axum 0.8.9 的 `merge` + `fallback` 语义实测更正。** 原先 `router.rs` / `business.rs` / `api/README.md` 三处都写了「业务路由带 fallback 会 panic」——实测**不会**：`merge` 的 `(true, false)` 臂静默采纳被并入者的 fallback，随后外层 `.fallback(not_found)` 又把它覆盖掉。也就是说，写了 fallback 的人不会收到任何信号，只会发现自己的 404 处理器永远不执行。禁令因此从"靠 panic 保证"改成"靠门禁扫描保证" | `api/src/{router,business}.rs`；`api/README.md`；`Makefile.project`：`business.rs` 里不得出现 `.fallback` |
| C-78 | ➕ | D59 的落点从 `app/src/assembly.rs` 位移到 `app/src/boot/assembly.rs`，与 `boot/env.rs`、`boot/gate.rs` 同层——三者都是"启动"这件事的组成部分，按"离进程有多近"排 | `app/src/boot/` |
| C-79 | ⚠️ | **两段式的签名与文档不同。** 文档写 `launch(self, ex: &Executors) -> Running`；落地是 `assemble(config, build, &Executors) -> Result<Assembled, BootError>` 做完 runtime 解析，`launch(self) -> Running` 不再收参数、也**不返回 `Result`**。理由：`spawn_on` 只在同名重复登记时失败，而一个没能 spawn 的 future 原地 drop 会让提交门以 `ack_dropped` 中止——那是一条已有且已测的失败路径，不需要第二个失败通道 | `app/src/boot/assembly.rs` 模块文档；`launch_is_the_only_place_that_spawns` |
| C-80 | ➕ | **`worker` 面是条件登记的。** C-42 只说了「`enabled` 由 app 在装配期读」，没说读完之后做什么：`worker.enabled = false` 时它**根本不进 supervisor**。另一种写法（照样登记、让面自己立刻返回）会得到一个在册、回执、随即 `Returned` 的面，而 §5.3 的 first-failure 会把那次正常返回当成关停理由——进程一起来就自己停了 | `app/src/boot/assembly.rs`；`a_disabled_worker_is_never_registered` |
| C-81 | ➕ | 新增 `Executors::for_tests`（`#[cfg(test)]`）。装配用例要一个只有主 runtime 的 `Executors`，而真造一个 `RuntimeSet` 必须在 async 上下文之外（`Runtime::new` 在 runtime 里 panic）。记为一次**为可测性开的测试专用构造函数**——它不出 crate，也不出 `cfg(test)` | `app/src/rt.rs::Executors::for_tests` |
| C-82 | ➕ | 重载**复用** `BootstrapError`，不另设一套重载错误。两条路径读的是同一个文件、同一批失败形态；分两套类型只会让"同一个错在启动期和重载期叫两个名字" | `app/src/config/reload.rs` |
| C-83 | ⚠️ | `reload` **不返回 `Result`**，返回 `ReloadOutcome`，并自己发那几条记录。重载失败**不是**进程失败——旧配置还在用，进程照常服务。返回 `Result` 会诱使调用方顺手 `?` 它，而那一个问号就把"配置文件写错了"升级成了"进程退出" | `app/src/config/reload.rs::{ReloadSource::reload, ReloadOutcome}`；`reload_with_missing_file_fails_and_keeps_last_good` |
| C-84 | ➕ | `ReloadSource` 在**启动时**就把路径与环境变量快照冻住，运行期不再重算。重算要么得到同一个答案，要么意味着有人在运行期改了进程环境——而 `set_var` 在 Rust 2024 里是 `unsafe`，本模板一次都没用。直接后果是一条必须写进 `README.project.md`「当前边界」的限制：**改环境变量要重启，SIGHUP 不管用** | `app/src/config/reload.rs::ReloadSource` 模块文档 |
| C-85 | ➕ | `read_limited` / `classify_read` / `utf8_vars` / `ReadError` 从私有放宽到 `pub(super)`：重载要复用启动期那一套读取，而不是再写一份。它们仍然出不了 `config` 模块 | `app/src/config/bootstrap.rs` |
| C-86 | ➕ | 重载路径上的钳位**并进 `config_reloaded` 这一条事件**，不另发。一次重载最多留一条记录——启动路径逐条 `warn!`，两边不重复（C-06 的另一半） | `app/src/config/reload.rs`；`a_clamped_value_is_reported_with_the_reload` |
| C-87 | ➕ | 主循环的升级（第二次 Ctrl-C）是**按段（stage）粒度**生效的，不是按轮：收紧后的 plan 在**下一段开头**现读，不截断正在跑的那一段。截断的话，一次 harvest 会在半途被打断，而它正在等的那个面既没收尾也没被 abort——它会落进 `unreaped`，于是"人多按了一次"变成了"关停不干净" | `app/src/lifecycle/loop.rs`；`a_second_stop_request_tightens_the_plan_without_abandoning_the_stage` |
| C-88 | ➕ | `RunReport` 有**第三种结局** `Printed`（`--help` / `--version` 这类"没跑起来，但也不是失败"）。只有成功与失败两种的话，`--version` 要么被算成一次成功的运行（于是"进程跑过吗"这个问题答错），要么被算成失败（退出码非零） | `app/src/lifecycle/report.rs::RunReport::printed` |
| C-89 | ➕ | **`app` 删掉了 `serde` 依赖。** 初稿的理由是"`deserialize` 这个动作发生在本 crate"，但 `serde_path_to_error::deserialize` 是自由函数，`app` 里没有任何一处需要把 `serde` 的名字写出来（`Config` 的派生在 `core`）。`unused_crate_dependencies` 在 lib target 上当场抓出来的——一条"用得上但写不出来"的依赖就是 §11 说的为观感加的依赖 | `app/Cargo.toml` 的注释 |
| C-90 | ➕ | 信号流外面包了一层 `Signals`。不包的话，流结束之后每次 poll 都立刻 `Ready(None)`，主循环的 `select!` 会变成**忙等**——CPU 打满，而日志上什么都看不出来。包装层让它报一次"流没了"，之后永远 pending | `app/src/lifecycle/loop.rs::Signals`；`an_ended_signal_stream_reports_once_and_then_hangs` |
| C-91 | ➕ | `fail_before_telemetry`：subscriber 还没装好就失败时，用**默认**遥测配置临时装一个把这条失败发出去。没有它的话，①～④ 的失败一条日志都没有——进程静默退出、退出码 1，使用者手上只有一个数字。装不上时（真的已经装过了）就只剩返回值本身，退出码仍然是对的 | `app/src/lib.rs::fail_before_telemetry`；`app/tests/bad_config.rs` |
| C-92 | ⚠️ | **`run` 收的是 `FnOnce() -> io::Result<S>` 而不是一条流。** 设计初稿写的 `run(env, stops: impl Stream)` 在这台机器上跑不起来：`tokio::signal::unix::signal()` 在 runtime 之外调用会 panic（§18.1 P-G 实测），而 runtime 是 `run` 自己建的。注入点没有变，变的只是注入的是值还是产生值的函数 | `app/src/lib.rs::run`；`app/src/main.rs` 模块文档 |
| C-93 | ➕ | 信号注册排在装配**之前**。注册之后、第一次 poll 之前到达的信号不会丢（实测），而装配可能很慢（建连接池、绑端口）——那段窗口里收到的 SIGTERM 若走默认处置会直接杀掉进程、跳过全部收尾。提前之后，那段窗口只剩 ①～⑥，且这几步都不做 I/O 等待 | `app/src/lib.rs` 模块文档 ⑦ |
| C-94 | ➕ | `StartFailure` 只为了在 `block_on` 内部统一 `?` 而存在，**不对外**。⑦ 与 ⑧ 失败的类型不同，但它们之后的处理完全一样；写两层 `match` 的话，两份「启动失败」处理迟早会分叉 | `app/src/lib.rs::StartFailure` |
| C-95 | ➕ | `app/src/main.rs` 与 `app/tests/*.rs` 顶上各有一条 `#![allow(unused_crate_dependencies)]`。这条 lint **逐 target 判定**，而那几个 target 只用到自家 lib；`app` 声明的其余依赖服务于 lib target，在那边照常被管着（`serde` 就是在那边被抓出来删掉的，见 C-89）。不写十几行 `use tokio as _;` 去哄它——那些语句不表达任何东西 | `app/src/main.rs`；`app/tests/{run,bad_config}.rs` |
| C-96 | ➕ | `ReloadOutcome` 的载荷带 `#[cfg_attr(not(test), allow(dead_code))]`：那些字段是**给断言看的**，产品代码只读结局不读载荷。用 `allow` 而不是删字段，是因为删了之后"重载报告了什么"就只能靠抓日志验 | `app/src/config/reload.rs::ReloadOutcome` |
| C-97 | ➕ | `StopState::is_stopping` 是 `#[cfg(test)]` 的。编排层从不问这个问题——它靠**自己在哪一段代码里**就知道答案（主循环里必然没停，`drive` 里必然在停）；给非 test 构建留着它等于留一个"可以跑去问状态机"的口子，而那正是把关停判断散开的第一步。用例需要它，因为用例是从外面看这台状态机的 | `app/src/lifecycle/stop.rs::StopState::is_stopping` |
| C-98 | ➕ | 主循环对四种重载结局**穷尽匹配**，没有 `_` 臂。加一种结局时编译器必须报错——一个 `_` 臂会让新结局静默走进"什么都不做"，而那正是重载类缺陷最常见的形态 | `app/src/lifecycle/loop.rs` |
| C-99 | ⚠️ | `cli.rs` / `lib.rs` 里"安装根"的措辞更正为「可执行文件**所在**目录」。原文写的是"可执行文件路径"，两者差一级——按原文实现会把配置找到二进制文件**里面**去（D23） | `app/src/lib.rs` ③；`app/src/cli.rs` |
| C-100 | ⚠️ | **`harvest` 返回前要把已经自己结束的任务收掉**（`try_join_next_with_id`，非 async，不花预算）。不收的后果不是少几条记录，是**结论错**——见下一条 | `core/src/task/supervisor.rs::harvest` |
| C-101 | ⚠️ | **`HarvestOutcome::forced` 重新定义**为「abort 真的掐到了还在跑的任务」，取自结果（`any Cancelled \|\| !unreaped.is_empty()`），不取自"调用过 `abort_all()`"。`reap` 段一律对剩余任务调 `abort_all`，按旧写法这一项**恒真**，于是 `succeeded()` 的六项合取恒假、**每一次干净关停的退出码都是非零**——而日志上看起来一切正常 | `core/src/task/supervisor.rs`；`a_cooperative_shutdown_is_not_reported_as_forced` |
| C-102 | ⚠️ | **`Phase::Forcing` 是可跳过的**：宽限段结束时册上已经没有任务就直接进 `Stopped`，一次干净的关停**不经过**它。§5.1 的状态机图因此改成 `[Forcing]`。不可跳过的话它就不是一个阶段，只是一行每次都发的固定日志 | §5.1 状态机图；`core/src/lifecycle.rs` |
| C-103 | ⚠️ | **`BuildInfo.package` 删除，`BuildInfo.service` 新增**，`current` 改成收 `service: &'static str`。`package` 报的是 crate 名（`<前缀>-core`），那是 workspace 的内部构造，发到对外契约上等于把仓库布局公开。入参钉成 `&'static str` 是为了把"只能来自 `env!`"从一句注释变成一条类型约束 | `core/src/build_info.rs` |
| C-104 | ➕ | `ProcessEnv.bin_name` 从 `String` 改成 `&'static str`，理由同 C-103，顺带省掉一次分配。收一个借来的串就等于允许有人拿 `argv[0]` 填它——那样一来，把可执行文件改个名就会换掉环境变量前缀和服务身份，一次改名变成一次静默的配置迁移 | `app/src/boot/env.rs` |
| C-105 | ⚠️ | **`/v1/info` 报 `service` 而不是 `package`**。C-55 那份"成功响应刻意避开信封键名"的清单里 `package` 随之改成 `service`。契约用例的夹具用一个**不等于任何 crate 名**的串（`RIG_SERVICE = "contract-rig"`），于是"退回去读 `CARGO_PKG_NAME`"当场红 | `api/src/system.rs`；`api/tests/contract.rs` |
| C-106 | ➕ | `config_loaded` 事件带上 `install_root`（D23 证据③）。它和 `path` 是同一个问题的两半——配置从哪来、数据会去哪，两者都锚在这个目录上（§5.9）。分成两条记的话，运维要对齐两个时间戳才能拼出一次启动的落脚点；而漏掉它的后果是最难查的那一类：进程起来了、日志干净、写的却是另一个目录下的库 | `app/src/lib.rs::replay`；`a_different_executable_moves_config_data_and_the_logged_root_together` |
| C-107 | ⚠️ | **补上 `storage_close_result` 事件。** D20 与 §12 / §15 三处都把它当证据用，落地时却漏了——`CloseOutcome` 一路进了 `RunReport` 决定退出码，却一条日志都不留。级别按结局分：`Closed` → `info`，`SkippedUnproven` → `warn`（"需要人来看"不是失败），`Failed` → `error` | `app/src/lifecycle/loop.rs` |
| C-108 | ⚠️ | **§15 的编排层场景不能全放在 `app/tests/`。** `run` 调 `set_global_default`，那个槽位每进程只有一个：第二次调用拿到 `AlreadyInstalled`，`run` 会把它变成一次 telemetry 启动失败——于是同一个二进制里的第二条用例验的不再是它想验的东西，**而且它会不会红取决于执行顺序**。同一条约束的另一半：`LogCapture` 装的也是那个槽位，所以「调 `run`」与「断言日志」在一个二进制里只能有一个。分法：要读日志的编排用例放 `app` 的**单元测试**（不经过 `run`），要跑完整条 `run` 的放 `app/tests/` 下**一个文件一条** | `app/tests/run.rs` 模块文档；`app/README.md`「一个测试二进制里只能有一次 `run`」 |
| C-109 | ⚠️ | 任务闭合事件定名 `task_exit`，不是初稿的 `task_exited`。事件名是**名词**不是句子：同一批里还有 `runtime_stopped` / `config_loaded`，混用时态会让过滤器写起来要记两套拼法 | `app/src/lifecycle/loop.rs`；`app/src/lifecycle/stop.rs::StopCause::as_str` |
| C-110 | ➕ | `a_default_config_creates_exactly_one_runtime` 拆成两条：它原先同时断言"只建了一个"和"关停只访问一个"。§4.5 代价表是逐行的，一个用例扛两行的话，红的时候读不出是哪一行的性质没了 | `app/src/rt.rs::single_runtime_shutdown_visits_exactly_one_runtime` |
| C-111 | 🔧 | **P6（不引入 Python）对设计稿的清理是成片的，不止 C-14 那一处。** D1 / D3 / D39 的落点原本都写 `scripts/template.py::structure`，D48 写 `::normalize_lock`，D49 写 `::matrix`，§11.4 的 `preflight` 还把"Python 版本检查"列成子项——那是照着 B 的形态抄下来的，而 P6 把 B 的实现语言整个否了。全部重指到 shell：`scripts/{structure,normalize-lock,matrix,gen}.sh`，`preflight` 改查 `cargo` / `cargo-generate` / `git`。**形态继承不变**（§11.1 那四行的「来源」列仍如实写 B 的 `scripts/template.py`，那是取证事实，不是落点） | D1 / D3 / D39 / D48 / D49 / D51 的落点列；§11.4 `preflight` 行 |
| C-112 | ⚠️ | **`include` / `exclude` 决定的是「渲不渲染」，不是「进不进生成结果」。** §11.3 把 `include` 和 `ignore` 并排放，读起来像一对互补的开关；实际上 `Matcher::should_include`（`src/include_exclude.rs`）只在 `Include`（过 Liquid）与 `Exclude`（逐字节拷贝）之间选，**两种结果都进生成结果**，真正决定进不进的只有 `ignore`。另两条同源语义：文件名走 `substitute_filename`，对 `Exclude` 的条目也照渲不误（`src/template.rs`）；`include` 与 `exclude` **互斥**，同时给出时 `exclude` 被静默丢弃只留一条 `warn!`（`src/config.rs`）。本模板只用 `include` | `cargo-generate.toml` 开头的三条判据 |
| C-113 | ⚠️ | **`ignore` 里写的是字面路径，不是通配符，而且不存在的条目静默跳过。** 实现是 `let mut p = PathBuf::new(); p.push(dir); p.push(f);`（`src/ignore_me.rs::remove_unneeded_files`），删除前再按 `Path::exists()` 过滤（`remove_dir_files`）。于是这份名单里拼错一个字、或者顺手写成 `**/*.md`，**不报错、不警告**，只是静默地什么都没删——症状是用户的项目里躺着本模板的设计稿。唯一能在用户侧拦住它的地方是后置钩子。实测：把 `"docs"` 改成 `"doc"` 后生成，post 钩子 exit=1 并指名 `docs` 泄漏 | `hooks/post.rhai::assert_absent`（五条：`docs` / `scripts` / `.github` / `rust-toolchain.toml` / `target`） |
| C-114 | ⚠️ | **B-27「`post.rhai` 每步校验返回值」这条纪律的形式错了，要改成「用 `file::exists` 验前置与后置条件」。** 理由来自 `src/hooks/file_mod.rs`：`file::rename` / `file::write` 失败自己就返回 `Err`，rhai 当场中止，**没有返回值可校验**；而 `file::delete` 对不存在的路径是 `if path.exists() { … }` 的**静默 no-op**，指望它报错是指望不上的。路径写错这一类错误，只有 `exists` 断言能抓到 | `hooks/post.rhai::promote` 的三处 `file::exists`（前置、冲突、后置） |
| C-115 | ⚠️ | **「`hooks/` 在 `post.rhai` 里自删」（§11.3）是错的；「`hooks/` 不进 `ignore`」这条结论对，但理由不是原文那条。** 0.24.0 的顺序是 pre-hooks → 按 `ignore` 删文件 → 渲染 → post-hooks → `remove_dir_files(全部 hook 文件)`（`src/lib.rs`）。于是：(a) `hooks` 一旦进 `ignore`，`post.rhai` 会在轮到它之前就被删掉——**这才是它不能进的理由**；(b) hook 文件由工具自己删，`post.rhai` **不需要自删**，它只需要把空掉的目录收走。`file::delete` 对目录走 `remove_dir_all`，而脚本已被 `eval_file` 整个读进内存编译完，删掉自己是安全的，工具随后那一步会因 `Path::exists()` 为假而空转 | `cargo-generate.toml` 的 `hooks` 注释段；`hooks/post.rhai` 末尾的 `file::delete("hooks")` |
| C-116 | 🔧 | **§11.3 的 `ignore` 里那条 `.genignore` 是多余的，删掉。** `cargo-generate.toml` / `.genignore` / `.cargo-ok` 三个文件是**无条件**被移除的（`src/ignore_me.rs::get_ignored` 的 `default_ignored`）。把它们写进 `ignore` 不会出错，但会让读的人以为"不写就会留下来" | `cargo-generate.toml` 的 `ignore` 列表 |
| C-117 | ➕ | **`include` 补 `**/README.md`，`ignore` 补 `rust-toolchain.toml`。** §11.3 的白名单只有 `README.project.md`，漏掉了六个成员各自的 README——它们正文里写的是 crate 的真名，不渲染就会把 `{{crate_prefix}}` 原样留给用户。`rust-toolchain.toml` 则是反过来：模板仓库钉工具链是为了让 `structure` 的逐字节比较可复现，把它塞给用户等于替他做了一个会过期的决定（DP6） | `cargo-generate.toml` 的 `include` / `ignore` |
| C-118 | ⚠️ | **`[placeholders]` 清空，`crate_prefix` 改为在 `hooks/pre.rhai` 里从 `project-name` 推导并校验。** §11.3 原本放了一条带 `regex` 的占位符。换掉它有两条理由：(a) 要问用户的只有"项目叫什么"，而 `--name` / 交互提示已经在问，再放一个占位符等于问两遍同一个问题，然后由模板来处理"两次答得不一样"；(b) 占位符只能表达"形状对不对"，表达不了"为什么不对"——名字写错时用户该拿到一句能照着改的话，不是一条正则。`--define crate_prefix=…` 仍然可用：pre 钩子先看 `variable::is_set`，显式给定就只校验、不覆盖 | `cargo-generate.toml` 的 `[placeholders]` 注释段；`hooks/pre.rhai` 的取值段 |
| C-119 | ⚠️ | **`project-name` 不能直接当 `crate_prefix` 用：`sanitize_project_name` 保留 snake_case。** 实现是 `if to_snake_case(name) == name { name } else { to_kebab_case(name) }`（`src/template_variables/project_name.rs`），所以 `--name my_service` 拿到的 `project-name` 是 `my_service`，**带下划线**。包名里混用两种分隔符（`my_service-core`）cargo 收，但锁文件反向映射和人眼都容易看错。`pre.rhai` 因此必须自己 `to_kebab_case` 再手写校验形状（rhai 没有正则）。实测：`--name my_service` → 目录 `my_service`、前缀 `my-service`；`--name MyService` → 工具先改名 `my-service`、前缀同；`--name a` 与 72 字符长名均通过（DoD 3 的极短 / 极长两组） | `hooks/pre.rhai::is_valid_prefix` 与取值段 |
| C-120 | ➕ | **钩子 abort 之后目标目录是空的，但已经存在——每条 abort 消息都得带一句"先把它删掉"。** `destination.create()` 在任何钩子之前就跑了，而渲染与钩子全发生在一个临时目录里，`copy_expanded_template` 是最后一步（`src/lib.rs`）。好的一面：中止不会留下半成品，用户目录里一个文件都没有。坏的一面：`ProjectDir::create` 见到已存在的路径就 `bail!("Target directory already exists")`，用户改完名字重跑，撞上的是这句与真实原因毫无关系的报错。实测：两次失败生成各留下一个 0 条目的目录 | `hooks/pre.rhai::reject`；`hooks/post.rhai::reject` |
| C-121 | ➕ | **模板树里不能有符号链接，这需要一条门禁扫描。** 生成结果落盘的最后一步 `copy_files_recursively`（`src/copy.rs`）**静默跳过符号链接**，只打一句 "Symbolic links not supported" 的 warning。当前模板一条软链都没有，所以现在是绿的；问题在于将来有人加了一条，症状是生成结果凭空少一个文件而全部门禁照绿。这条得在模板仓库侧扫 | `scripts/structure.sh::no-symlink`（已落地） |
| C-122 | 🔧 | **生成结果的全部结构纪律从 `Makefile` 的 shell 搬进一个 Rust 集成测试，由 `make test` 跑。** 三条理由，每条都是这个决定单独成立的原因：(a) **平台**——BSD grep 没有 `-P`，macOS 自带的是 GNU make 3.81（没有 `.ONESHELL`），BSD `sed -i` 要带空参数。「门禁在我这儿是绿的」不是一个能随代码发布的结论；(b) **依赖面**——§11.4 那句「生成结果的依赖面只有 Rust 工具链本身」就此逐字成真，不需要在括号里补例外；(c) **失败消息**——集合比对要说清"多了什么、少了什么"，`diff <(…) <(…)` 给不出来，`assert_eq!` 给得出来。代价是这些扫描要自己写剥离与词边界，代价与理由写在那个文件的头注释里。`.rs` 不进渲染面这件事在这里帮了忙：成员**目录**名是固定的（`core`/`storage`/…），只有包名带前缀，所以扫描器里一个模板变量都不需要 | `testkit/tests/discipline.rs`（20 条）；`Makefile.project` 的 `test` 目标。重指 C-19 / C-31② / C-58 / C-64 与 §11.4 原先写 `structure` 的那一行 |
| C-123 | ⚠️ | **六个成员 README 与根 `Cargo.toml` 原本指名 `make check` 的 `structure` 门禁，而那个目标只存在于模板仓库——一条活的 D56 违例。** 这七个文件都在 `include` 白名单里，会原样交到用户手上，于是用户按文档去跑一个他那儿根本没有的检查。全部改指 `test` 门禁并写明实现文件 | 六个 crate 的 `README.md`「公共出口」段；根 `Cargo.toml` 的 `[workspace.dependencies]` 注释 |
| C-124 | ➕ | **根 `Cargo.toml` 那一条不是改措辞就够：生成结果原本根本没有分层邻接表门禁。** 注释宣称「多一条少一条都红」，而能判红的只有模板仓库的 `structure.sh`。与其把话说软，不如把门禁给它——新增一条用例，把 §3.3 的表逐行抄成常量，两侧排序后比集合。**表里的顺序是分层顺序（`core` 在前、`api` 在后），清单里的顺序是作者写下的顺序，两者没有理由一致**，所以比的是集合不是序列——这一点是被自己的第一次红指出来的 | `testkit/tests/discipline.rs::the_dependency_edges_equal_the_layering_table` |
| C-125 | 🔧 | **D1 / §3.3 的判据在生成结果侧改写为清单文本扫描，模板仓库侧保留 `cargo metadata --filter-platform` 的已解析视角。** 原判据要派生进程，而 DP9 定了生成结果不做进程测试。两侧各自看得见什么写在测试的文档注释里，不含糊：清单文本**看不见** feature 激活出来的边（barter-rs 里 `barter-integration → barter-instrument` 的反向流就是这么发生的），已解析图看得见；反过来，手工加进清单的那一行是文本扫描当场就抓的，而那才是用户侧真实会发生的事 | `testkit/tests/discipline.rs::the_dependency_edges_equal_the_layering_table` 的文档注释；`scripts/structure.sh` 保留 feature 解析视角 |
| C-126 | 🔧 | **D5 的判据同样从 `cargo metadata` 的 `kind == ["bin"]` 计数改写为文本扫描，并且多扫两项。** 同一条 DP9 理由。改写之后反而更严：除了断言只有一处 `[[bin]]` 声明，还断言只有一个 `src/main.rs`、且没有任何 `src/bin/` 目录——后两者是 cargo **自动发现**的，一个只读清单的判据看不见它们，而"顺手加一个 `src/bin/tool.rs`"正是第二个可执行入口最常见的来法 | `testkit/tests/discipline.rs::the_workspace_has_exactly_one_executable_entry_point` |
| C-127 | ➕ | **D4 的两侧比对需要一对显式标记（`<!-- exports:begin -->` / `<!-- exports:end -->`），不能让扫描器去猜 README 里哪些反引号算数。** 具体触发它的是 `worker/README.md`：那里有一张 `SPEC` / `build` 的表，两项都是 `TickerPlane` 的**关联项**，不是 crate 出口。任何"扫全文里的反引号标识符"的做法都会把它们算进来，然后写文档的人就会被迫为了哄门禁而改文档结构。标记还顺带承载 feature 语义（`feature=<name>`），而 `storage` 那两段是**分开比**的（C-31②），不取并集 | 六个 crate 的 `README.md`；`testkit/tests/discipline.rs::every_public_export_is_listed_in_the_crate_readme` |
| C-128 | ⚠️ | **`MAIN_RUNTIME_NAME` 与 `RuntimeThreads` 漏在 `core/README.md` 的清单之外**，由 D4 门禁的第一次运行抓到。这正是这条门禁存在的理由的实例：两个名字都是在写 `app` 的过程中补进 `core` 的出口的，而补出口那一刻没有人回头看 README | `core/README.md` 的 `config` 段 |
| C-129 | ➕ | **全部源码扫描共用一条剥离通道（去注释、去 `mod tests` 之后的一切），而这条通道的前提本身是不变量，要有人守。** 前提有三条：仓库里不存在 `/* */` 块注释；`mod tests` 永远在第 0 列；`mod tests` 永远是所在文件的最后一个顶层项。三条都成立时剥离是精确的，破一条就会开始静默漏扫——漏扫的表现是绿色。所以把它们写成扫描清单里的第 0 条用例 | `testkit/tests/discipline.rs::the_source_tree_keeps_the_shape_every_other_scan_assumes` |
| C-130 | ⚠️ | **文本扫描必须带词边界，这是实测出来的，不是防御性写法。** 两个真实例子：`compile_error!(` 的尾巴就是 `error!(`，`notify_for(` 的尾巴就是 `for(`。第一个在门禁的第一次运行里就把 `app/src/signals.rs` 的 `#[cfg(not(unix))]` 分支报成了"一条没有 `name:` 的事件"。写下来是因为**门禁第一次运行就指着一条无关的行报红**最常见的下场不是有人去修扫描器，是有人把判据改松 | `testkit/tests/discipline.rs::word_start` 的文档注释 |
| C-131 | ➕ | **每条扫描都要断言自己确实扫到了东西，用下界不用精确值。** 一条匹配不到任何东西的扫描和一条全都通过的扫描，在报告上长得一模一样：绿。用精确值的话，每加一条日志、每加一个 `select!` 都会让门禁红一次，而那种红没有信息量，只会训练人把数字调大——下界只回答一个问题：针脚还认得出代码吗。当前下界：`select!` 6、tracing 事件 40、`#[from]` 3、公共出口 80 | `testkit/tests/discipline.rs::assert_saw` |
| C-132 | ⚠️ | **D57 的分支计数器会把 `impl Trait for Type` 数成一个分支**（`for` 是关键字表里的一项）。同理 `for<'a>` 的高阶生命周期。修法是跳过以 `impl ` 起头的行并对 `for<` 回退一次；写进台账是因为这条假阳性只在"某个适配器文件里恰好有一个 `impl`"时才现形，而那时门禁指着的是一行与判据毫无关系的代码 | `testkit/tests/discipline.rs::branch_count` |
| C-133 | ➕ | **D2「成员清单不自己定版本」的扫描要给 `service-*` 路径别名开一个豁免。** 那五条是 `{ workspace = true }` 的转写，写的是 `package = "…"` + `path = "…"`，没有 `version =`，但形态上不是 `workspace = true`。豁免按**别名前缀**记，不按行号记 | `testkit/tests/discipline.rs::member_manifests_never_decide_a_version_themselves` |
| C-134 | ➕ | **R8 的「`make dev-config` 之类的一键准备」落成一个具体目标，且它抄文件而不是跑一次进程。** 进程自己写模板发生在启动路径的中段，而想在**启动之前**改端口的人拿不到那个时机——这正是第一次用它的人最常见的需求。抄的是 `app/assets/service.toml`，也就是二进制里 `include_str!` 进去的那一份，两者逐字节相同，不存在第二个真值。已存在时不覆盖：一个会把改好的配置默默换回出厂值的命令比没有这个命令更糟 | `Makefile.project::dev-config`；`README.project.md`「配置和数据在 `target/debug/` 下」 |
| C-135 | ➕ | **`Cargo.lock` 的产生方式（DP5 / D48）实测定型。** 模板根的 `Cargo.toml` 带 `{{crate_prefix}}`，cargo 解析不了，所以不能就地 `generate-lockfile`。流程是：把模板树展开成一个具体名字的工作区 → 在那里 `cargo generate-lockfile` → 把六个成员名反向替换回 `{{crate_prefix}}-*` → 落到模板根。**不重排条目**：实测 `--locked` 比的是解析结果，不是文件里的行序，渲染后成员名的字典序变了也不影响——`demo-svc`（成员块在文件第 245 行一带）与 `tplxlock`（在第 1598 行一带）两棵树都过了 `--locked`，名字矩阵那四组又各证一次。当前锁定 **189 个第三方包**（锁文件共 195 条 `[[package]]`，含六个成员） | 模板根 `Cargo.lock`；`scripts/normalize-lock.sh`（已落地，幂等与负向用例均已实跑） |
| C-136 | ➕ | **D51「生成结果里不得残留未展开的 `{{ … }}`」的豁免清单实测是四个 `.rs`，全部是 Rust 自己的 `{{` 转义**：`core/src/config/expand.rs`（`#[error]` 格式串里的 `${{` 与 `${{NAME}}`）、`app/src/cli.rs`（usage 文本里的 `${{PREFIX}}_CONFIG`）、`testkit/tests/discipline.rs`（失败消息里的 `key = {{ … }}`）、`api/tests/contract.rs`（`format!` 里的 JSON 字面量）。四条都不是模板变量。所以 D51 的扫描判据要写成「`{{` 的出现位置集合 == 这份清单」，相等比较 | `scripts/gen.sh` 的审计段（已落地） |
| C-137 | ⚠️ | **`README.project.md` 里嵌 `{{crate_prefix}}` 的行不能参与列对齐。** 原先三段配置优先级写成"名字 + 空格补齐 + 说明"的两列块，`my-service` 展开后列就歪了，极长名字（DoD 3 的那一组）会歪到没法读。改成"说明在前、名字在后"，于是变长的那一项永远落在行尾。同一条约束适用于目录树块与 `curl` 那几行的行尾注释——已逐条核对 | `README.project.md`「配置」段；`scripts/matrix.sh::no-padding`（已落地，把它变成每组都跑的判据——见 C-145 第 ③ 条）；名字矩阵四组已实跑 |

| C-138 | ⚠️ | **cargo-generate 0.24.0 不看模板自己的 `.gitignore`。** 实测：往模板的 `.gitignore` 里加一行，再把同名文件放进模板根，生成结果里**照样有**这个文件。两个方向的后果都要知道。好的一面：不会出现"有人按库的习惯把 `Cargo.lock` 写进 `.gitignore`，于是锁文件静默不发布、每个生成项目的第一条命令都红在 `--locked`"这种坑——很多 Rust 模板的 `.gitignore` 抄的正是库的习惯。坏的一面：模板目录里的任何脏东西都会原样进用户的项目，而**唯一**的删除机制是 `cargo-generate.toml` 的 `ignore`（字面路径、拼错静默无效，C-113）。所以那份名单不只是"模板自己的东西"，它同时是垃圾过滤器 | `cargo-generate.toml` 的 `ignore` 注释；本条由 C-139 触发 |
| C-139 | ⚠️ | **`.DS_Store` 实测泄漏进了生成结果。** 模板根有一个（Finder 留的，已被 `.gitignore` 挡在提交之外），于是从本地路径生成的项目根目录里就多了一个它。`.gitignore` 挡得住提交，挡不住拷贝——C-138 说明了为什么。修法分两层：`ignore` + `assert_absent` 各加一条（管仓库根，这是它出现的绝大多数位置，代价为零），子目录里的由模板仓库的 `structure` 门禁递归扫（`.DS_Store` / `Thumbs.db` / 编辑器目录）。**只从 `--git` 生成的用户本来看不到这个 bug**，因为它从未被提交——也就是说这条只有模板作者自己的 `--path` 门禁能发现，而那正是逐字节比较跑的那条路径 | `cargo-generate.toml` 的 `ignore`；`hooks/post.rhai::assert_absent`（现在七条）；`scripts/structure.sh::no-junk` 的递归扫描（已落地） |
| C-140 | 🔧 | **B-32 的原修法「`.gitignore` 等随交付文件改用英文」不采纳，采纳的是它指出的问题。** 问题是真的：随交付的文件不该讲模板仓库自己的事。但"改用英文"这个修法与 F4（中文文档与注释）直接冲突，而且它修的是表象——一个用英文写的、引用了模板设计稿节号的注释，一样是违例。判据按 C-67 重新定义为**扫模板自身的痕迹**（仓库路径、备份分支名、本设计稿的节号与 B-xx/C-xx 编号），语言不在判据里。于是 `.gitignore` 的注释是中文的，和 `Makefile`、README、各成员 README 一致——生成结果拿到手的是一份语言统一的文档面 | `.gitignore`；§11.2 表中「`.gitignore` 等随交付文件改用英文」一行以本条为准 |
| C-141 | ➕ | **`.gitattributes` 只写一条 `* -text`，并且不进生成结果。** B-33 的原修法是"覆盖参与字节比较的全部文件类型"，而"类型清单"这个形态本身就是它出问题的原因——上一版列了清单然后漏了 `*.md` 和 `Cargo.lock`，而这两样恰好都参与比较，漏了之后的失败是静默的。通配规则没有"漏一类"这种失败模式。真正需要它的是 `make verify-git`（D50）那一趟：cargo-generate 会另起一个 checkout，两个 checkout 各自套用本机的 `core.autocrlf`，于是"同一个提交"在两台机器上可以有不同的字节，而门禁比的正是字节。同时**不写 `export-ignore`**：删文件的机制只能有一个（`ignore` + post 钩子复核），第二个只在 `git archive` 下生效、门禁永远不走的机制，就是 B-25「覆盖率为零」的来法 | `.gitattributes`；`cargo-generate.toml` 的 `ignore`；`hooks/post.rhai::assert_absent` |
| C-142 | 🔧 | **D3 的「`tracing-subscriber` 只在 `app`」落地成一个两元素的允许集 `{ app, testkit }`。** 采纳的是 D3 的结论，不是它的字面：纪律管的是**生产图**——组合根之外没有谁能装全局 subscriber。`testkit` 从构造上就不在生产图里（它是五个成员的 dev 依赖、不是任何人的 normal 依赖，同一个脚本的邻接表相等比较已逐格验过），它里面的 subscriber 是测试用的私有捕获器，进不了任何二进制。把 `testkit` 从允许集里去掉的代价是日志断言要么消失、要么在四个 crate 的测试模块里各抄一份，而后者正是 `testkit` 存在的理由（§3.3 的 `testkit` 行）。同一段里 `rt-multi-thread` 的判据也随之定型：它在 `core` / `storage` / `api` **三份清单的 dev-dependencies 里都合法地出现过**，所以只能从去掉 dev 边之后的 feature 图上判，实测归属集是 `{ app }` | `scripts/structure.sh::third-party`（`declare_owner` 的允许集写成集合，每条集合带理由） |
| C-143 | ➕ | **依赖断言全部从 cargo 解析之后的图上读，不 grep 清单文本，而且不经 JSON。** 文本视角有两个它回答不了的问题：`[workspace.dependencies]` 的继承与 feature 的并集只在解析结果里成立；`[dependencies]` 与 `[dev-dependencies]` 在 grep 眼里是同一种东西（C-142 那三处 dev 的 `rt-multi-thread` 就是这个失败的实例——朴素 grep 判红之后，人会去放宽判据，最后放宽到真违例也拦不住）。读图用 `cargo tree --prefix depth`：它把层级变成行首的十进制数字，于是解析只要 `sed`，不需要 `cargo metadata` 的 JSON，门禁的依赖面仍然只有「Rust 工具链 + POSIX shell + git」（C-111）。三种取法：`-e normal` / `-e dev` 拿邻接表，`-e features --invert <包>` 拿已解析 feature 集，`-e no-dev,features -p <成员>` 拿去掉 dev 边的正向 feature 图。最后一种用正向而不是 `--invert`，因为反向图的 `(*)` 会折叠已打印的子树，"谁最终打开了这个 feature"可能正落在折叠掉的那段里 | `scripts/structure.sh` 头注释「它验的是图，不是文本」 |
| C-144 | ⚠️ | **与依赖图撞名的项目名会让随模板发布的 `Cargo.lock` 当场失配，而报错不提名字。** 实测（cargo 1.95.0）：`cargo generate --name axum` 生成的工作区里有一个本地包叫 `axum-core`，而 registry 的 `axum-core` 也在图里；两个同名包并存时 cargo 把锁文件里的依赖引用从裸名 `"axum-core"` 改写成带版本的消歧形式 `"axum-core 0.5.6"`。发布出去的那份锁是在不撞名的前缀下生成的，里面是裸名，于是不再匹配——而生成结果的三条门禁全带 `--locked`，用户敲的**第一条命令**红在 `cannot update the lock file ... because --locked was passed`，一句话都没提他的项目名。修法是 `hooks/pre.rhai` 在生成任何文件之前 `abort`，错误消息把机制讲完并给出完整的被占名单。名单是从锁文件**反推**的：取所有以那六个后缀结尾的包名去掉后缀，实测为 `axum` / `futures` / `sqlx` / `sqlx-macros` / `tracing`（已逐个验过 `axum`、`sqlx`、`tracing` 确实红，`tokio`、`orders-gateway` 正常）。它**会随依赖集变化**，而依赖集变的那天没有任何东西会提醒谁回来改钩子——所以模板仓库的 `structure` 门禁重算一遍再和钩子里的清单做相等比较，与 `assert_absent` ↔ `ignore` 同一种两名单交叉比对（C-113 那条纪律的另一个实例） | `hooks/pre.rhai::collides_with_dependency`；`scripts/structure.sh::name-collision`（已落地，负向用例已实跑） |
| C-145 | ⚠️ | **名字矩阵第一次跑就打出三条真缺陷，全部只在某一类名字下可见。** 这是 D49 的价值本身，逐条记下来：① **`tokio` 组**——`structure.sh` 判断"一条依赖边是不是本地成员"原本用的是"包名以 `<前缀>-` 开头"，前缀取 `tokio` 时 `tokio-util` 这个正当的第三方依赖被认成一个叫 `util` 的成员，`storage` 行凭空多出一条边。判据改成**逐个列出六个成员名**（从 `$MEMBERS` 拼正则，不另抄一份）。② **`x` 组**——单字符前缀让任何裸子串匹配失效：`no-padding` 的首版在 `expect_used` / `unused_extern_crates` / `0002_xxx.sql` / `axum::extract` 上报了五条假红。两处收窄：只扫**渲染面**（逐字节复制的文件里不含项目名，内容不随名字变，不可能歪）+ 要求词边界。③ **50 字符组**——根 `Cargo.toml` 的成员别名表把 `path = …` 对齐在 `package = "<前缀>-storage"` 之后，那是一个长度随名字变的字段，补齐的列会跟着歪。这是 C-137 那条纪律在 README 之外的又一个实例，说明它不该只写在 README 那一行上，于是 `no-padding` 把它变成了一条**每组都跑**的自动判据 | `scripts/structure.sh::adjacency` 的 `MEMBER_RE`；`scripts/matrix.sh::no-padding`；根 `Cargo.toml` 成员别名表（`path` 不补齐，注释写了理由） |
| C-146 | ➕ | **复合入口只锁一次，安静与否是输出层的事。** `matrix` 要连着调用 `gen` 和 `structure` 各四遍，而那两个各自都要独占 `$GATE_ROOT`。不处理的话父进程持锁、子进程等锁，等满 600 秒然后红——红的原因与被测的东西毫无关系。修法是让 `gate_lock` 可重入：持锁者 `export GATE_LOCK_HELD=1`，子进程见到就直接返回。判据是"持锁者的子进程本来就在同一段临界区里，它们不是竞争者"。同一次改动把 `--quiet` 从"每个检查点一个 `if`"移进 `say` / `info` / `ok` / `gate_begin` / `gate_end`：检查点关心的是过没过，不关心谁在看；反过来写的代价是每加一项检查就多一处可以忘记加的条件，而忘记的表现是"安静模式下多一行"，没人会为此打红。`die` 不受安静模式影响——安静的是进度，不是结论 | `scripts/lib.sh::gate_lock` / 输出段；`scripts/matrix.sh` 的单次 `gate_lock` |
| C-147 | 🔧 | **D48 的「只映射 `cargo metadata` 里的本地包」落不了地，换成结构判据。** C-111 把 Python 从依赖面上删掉之后，门禁只剩「Rust 工具链 + POSIX shell + git」——而 `cargo metadata` 只吐 JSON，没有 `jq` 就得在 shell 里手写 JSON 解析，那是 C-143 已经拒绝过一次的路。真正的判据不需要 `cargo metadata`：`Cargo.lock` 自己就分好了类，一个 `[[package]]` 块**没有 `source =` 行**就是本地路径包。`awk` 十行读完，判据还更强——它读的是**这一份锁文件**里的事实，而 `cargo metadata` 读的是清单，两者在"锁文件被手改过"的情形下会给出不同答案，而那正是要拦的情形之一。替换本身是整串带引号全等匹配（`"tokio-core"` 换、`"tokio-util"` 不换），A-31 那条无差别前缀替换因此在实现层面就不可能发生。三条断言依次收口：`local-set`（本地包集合 == 六个成员，相等比较）→ `denormalize`（一次性前缀零残留 + 第三方 `name =` 集合不变）→ `round-trip`（新锁重新生成一次过 `--locked`）。实跑：幂等（第二趟逐字节相同）、负向探针（前缀 `tokio` + 第三方 `tokio-util`，`tokio-util` 未被触碰） | `scripts/normalize-lock.sh::lock_local_packages` / `::lock_denormalize`；D48 的判据列 |
| C-148 | ⚠️ | **解析用的一次性名字决定了六个成员在锁文件里的位置，所以它一旦定下就不能顺手改。** cargo 按名字字典序写 `[[package]]` 块，也按名字字典序写块内的 `dependencies`。换一个首字母不同的一次性前缀，六个块连同十几处引用会整体挪位，而**解析结果一个字节都没变**——实测从 `demo-svc` 换到 `tplxlock` 产生 156 行纯位移。那种 diff 读不出任何信息，却会盖住真正的依赖变动。反过来这也证了 C-135 的「不重排条目」：位置差这么远的两份锁，`demo-svc` 与 `tplxlock` 两棵树都过了 `--locked`，名字矩阵四组又各证一次——`--locked` 比的确实是解析结果，不是行序 | `scripts/normalize-lock.sh::LOCK_SCRATCH` 的冻结说明 |
| C-149 | ⚠️ | **`panic = "abort"` 的负向探针必须带 `--target <host-triple>`，否则它红在一个与被测纪律无关的地方。** 不带 `--target` 时 cargo 用同一份 profile 去编译**宿主工具**（build script、proc macro），而 proc macro 必须以 unwind 编译——编译器自己要在展开失败时收住。于是负向那一趟会先在 `serde_derive` / `thiserror-impl` 上炸掉，报错里一个字都不提 `compile_error!`，而这条门禁的全部意义恰恰是"失败文本必须指向那条守卫"。带上 `--target` 之后宿主工具走自己的那一套，被测的只剩目标产物。正向那一趟也带，为的是两趟落在同一棵 target 子树下、第三方依赖只编一次。三元组由 `rustc -vV \| sed -n 's/^host: //p'` 自报——写死一个等于把这条门禁钉在一台机器上，而它本该在任何能跑 Rust 的地方都成立 | `scripts/release-probe.sh::HOST_TRIPLE`；DP2 的落点段 |
| C-150 | 🔧 | **探针 example 注入生成树，不进模板交付面。** DP2 要产物级证据就得有一个真的会 panic 的 `main`，而它只有门禁一个消费者：放进 `<crate>/examples/` 会违反「不预铺没有消费者的东西」，放进 `scripts/fixtures/` 则要给 `structure.sh::absent` 多加一条名单项、并在模板树里留下一个每次有人看见都要先判断"这算交付物吗"的 `.rs`。做法与 `migration-rebuild.sh` 注入探针迁移一致：同一次运行里写入、用完随生成树删掉。程序与它的期望输出隔着二十行写在同一个脚本里——改了打印就必须改期望，反过来也一样，没有第三个地方需要同步。附带一条：example 是独立**目标**，`unused_crate_dependencies` 会为 core 那四条只给 lib 用的依赖各报一条警告；探针里 `#![allow(unused_crate_dependencies)]` 掉，因为构建失败时这份日志会被整段贴给人看，四条无关警告会把真正的错误挤到看不见的地方 | `scripts/release-probe.sh` 的探针段 |
| C-151 | ⚠️ | **门禁必须钉死 `LC_ALL=C`，且正则里不许出现多字节字符类。** `第[一二三四五六七八九]阶段` 这条历史标记分支写完之后连着几轮全绿，实际上它**一个真实输入都匹配不上**：脚本跑在非交互 shell 里，`LANG` 为空、`LC_CTYPE=C`，而 C 下的 `[…]` 是**字节**集合——它要求 `第` 与 `阶段` 之间恰好一个字节，中文数字是三个。分支死了和检查通过在输出上完全一样，这正是 `tooling-test` 要拦的那一类。发现它靠的是把这条判据喂给合成输入；之前一直没露头，还因为交互 shell 里 `grep` 被一个指向 `ugrep` 的函数遮住了，而 `ugrep` 按字符处理——同一条正则在手敲和在 `make` 下跑出两个结果。两处落点：`lib.sh` 顶部 `export LC_ALL=C`（顺带把 `sort` / `comm` 的排序规则钉住——多处相等比较靠两边同一套排序，`zh_CN.UTF-8` 下 `comm` 会对着它认为没排好序的输入给出错误差集而**不一定报错**），判据改用分支 `第(一\|二\|…\|九)阶段`。选 C 不选某个 UTF-8：cargo 与 git 都按字节序排，而且此前全部实测都是在 C 下取得的——钉 C 是保持行为，钉 UTF-8 是改行为 | `scripts/lib.sh` 的语言环境段；`scripts/audit-rules.sh::AUDIT_HISTORY_RE`；`scripts/tooling-test.sh::history-shape` 的 `第三阶段` / `第一阶段` 两条用例 |
| C-152 | 🔧 | **cargo-generate 0.24.0 的 `--test` 是第三条展开路径，它的四条实测语义决定了 `verify.sh` 能断言什么。** ①它把 `$CWD` 当模板原地展开，并在 `$CWD` 里建一个目标目录；②`--name` 与 `--destination` 在这条路下**都被忽略**，项目名来自它自己的随机词表（实测 `sour-sand` / `inquisitive-arch` / `stingy-request` / `hallowed-growth`）；③`CARGO_GENERATE_TEST_CMD` **不过 shell**，按空白切成 argv，所以它只能是「一个程序加参数」，`make check` 正好合适；④**它打印的 `Running "cargo test" ...` 是写死的常量**，命令换成 `pwd` 那行照样这么写。第 ④ 条直接否掉了一个看起来最自然的判据——拿那行断言「跑的是 make check」等于断言一个常量。改用只有生成结果的 Makefile 才打得出来的三行（`── fmt ` / `── lint ` / `✓ check 通过`）；①决定了必须先把模板树复制走再跑，否则模板仓库的写入口就不止 `make lock` 一个 | `scripts/verify.sh` 的头注释与 `expand-test` / `random-name` / `project-gate` 三段 |
| C-153 | ⚠️ | **`GIT_CONFIG_GLOBAL` 能影响 cargo-generate 的克隆，命令行 `--gitconfig` 不能。** 要给 `.gitattributes` 的 `* -text`（C-141）一条可观察的效果，就得在克隆方注入 `core.autocrlf=true`。两条路都试过：`--gitconfig <file>` 对 checkout 的换行转换**无效**，`GIT_CONFIG_GLOBAL=<file>` **有效**——不实测就写，会得到一条永远绿而且永远无意义的断言。配套的纪律是这条断言必须带负向对照：拿掉 `.gitattributes` 之后同一趟**必须**出现 CRLF（实测 89 个文件），否则「没变脏」既可能是 `* -text` 起了作用，也可能是这台机器压根不做转换 | `scripts/verify-git.sh` 的 `crlf-neutral` / `crlf-negative` 两段 |
| C-154 | ⚠️ | **抽取器错在两个方向上，而这两个方向不对称：多抽是吵的，少抽是静的。** 这条纪律是被自己的实现教会的——第一版判据写成 `#\[(?:tokio::)?test\]`，要求属性**恰好**是那两个字面量，于是 `#[tokio::test(start_paused = true)]`、`#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` 这类带参数的属性一个都抽不到，全树 **12** 条用例凭空消失，且消失的正是关停编排与 ticker 那批最吃重的用例。两侧因此**仍然相等**（文档清单也是拿同一个抽取器建的），`acceptance-ids` 全绿——这正是这条门禁存在的理由所要挡的那种绿。数字留在这里：真实用例 **264**；`grep -c '#\[test\]'` 数出 192，两个字面量数出 255，放宽成前缀数出 267（264 + 3 处散文）——三个数都不对，且错在相反的方向上。修法不是把属性形态一个个枚举进去（枚举永远漏下一种），而是**武装条件只看行首是不是测试属性，括号里写了什么一概不管**（`^\s*#\[(?:tokio::)?test\b`；`\b` 顺带挡掉 `#[test_case]`），并且对剩下唯一认不了的形态——参数被折成多行——**当场非零退出**，而不是静静跳过。多抽那一侧的触发者是散文：树上有三处 `#[test]` 写在文档注释里讲 `clippy.toml` 的 `allow-*-in-tests` 豁免，而最危险的形态恰恰最常见——提及落在文档注释的**最后一行**，下一行就是一个真函数（`worker/tests/ticker.rs` 的 `republish_interval`、`api/tests/contract.rs` 的 `send` 逐字如此），去掉行锚定就会凭空长出这两个 ID，文档侧只能补两条假条目才能变绿。探针（rustfmt 1.95.0 / edition 2024）：`#[test] fn f(){}` 会被拆成两行，尾随的 `// 注释` **原样保留**——所以一行式在 `cargo fmt --check` 绿的树里不可达，尾随注释式可达，判据必须认后者。最后一条是跨文件复位：`perl -ne` 的状态跨文件不清，上一个文件末尾若停在「已武装」，下一个文件的第一行会被算成它的用例，那条 ID 的出处会指向一个根本没有它的文件 | `scripts/audit-rules.sh::audit_test_names`（那次失败逐字记在函数头注释里）；`scripts/tooling-test.sh::acceptance-extract`：六条该抽 + 六条不该抽 + 一条该红，三个方向各跑过反例——把武装条件退回字面量，两条带参数的用例当场从集合里消失 |

**门禁现状（六个成员全在内）**：在 `{{crate_prefix}}` 展开为 `demo` 的树外工作区实跑，cargo 1.95.0 / 锁定 186 个包。

| 趟次 | fmt | clippy `-D warnings` | test |
| --- | --- | --- | --- |
| 默认 feature | 干净 | 干净 | **240 条全绿**（`core` 93 + `storage` lib 20 + `storage` 契约 3 + `testkit` 11 + `worker` 契约 7 + `api` lib 13 + `api` 契约 16 + `app` lib 75 + `app` 契约 2） |
| `--features demo-storage/test-utils` | 同上 | 干净 | **244 条全绿**（`storage` lib 22 + 契约 5，其余不变） |

doctest 两趟都是 0/0/0（六个 crate 各一行）。D4 的公共出口两侧集合相等已对 `core`、`testkit`、`worker`、`api`、`app` 各验一次，对 `storage` 按默认 / `test-utils` 两个视角各验一次（shell `diff`，差异 0 行）。

`app` 的 `[[bin]]` target 编出来是 `demo`，`cargo test` 对它报 0 条——它只有一个无分支的 `main`，全部分支在 lib 里（D58）。

**生成结果侧的实跑（与上表不是同一次测量）**：上表跑在手工展开的树外工作区上，那时模板还没有 `Cargo.lock`；这一趟跑在 `cargo generate` 的真实产物（项目名 `my-service`）上，三条门禁都走 `--locked`，锁定 189 个包。

- `make check` 全绿：`fmt` 干净、`clippy --workspace --all-targets -- -D warnings` 干净、两趟 `test`（默认 + `--features my-service-storage/test-utils`）全绿、doctest 计数 0。
- 结构纪律 20 条全绿，包含最后转绿的 `the_readme_names_make_check_as_the_only_gate`。
- `make dev-config` 写出配置后再跑一次不覆盖。
- `cargo run` 起得来；`/healthz` → `{"live":true}`、`/readyz` → `{"ready":true}`、`/v1/info` 带 `service` / `version` / `uptime_seconds`；未知路由 404 且信封形状正确。
- Ctrl-C 之后在册的每一个面各有一条收尾记录（`ticker`、`http`）、存储池关闭一条、`runtime stopped` 一条，退出码 0。

这一趟是**手工**跑的，不是自动化证据——`tokio::signal` 的投递、OS 观察到的退出码都不在任何测试的断言里。Phase 3 已照此办理：`docs/acceptance.md` 开头把十四条无自动化证据的事实点名一遍，理由与残余风险逐条写在 `docs/verification.md` 第 4 节，那一趟手工观测（连同一次 SIGHUP 热重载与一次重载失败）的原始记录在同一份文件的第 3 节。

---

*闸门 1 已通过（P1–P6 已裁决）。Phase 1.5 已完成并登记修订。Phase 2 已完成：§15.2 除 `--git` 时序外已结案（该条其后亦已证成立，见 §15.2 表末），修订见 §18；`core`、`testkit`、`storage`、`worker`、`api`、`app` 六层逐层走完四步（代码 → 该层门禁绿 → crate README → §18.2 修订）；工程链全部落地——`cargo-generate.toml`、`hooks/{pre,post}.rhai`、`Makefile.project`、`README.project.md`、`Cargo.lock`、`scripts/`、模板仓库的 `Makefile` 与 `README.md`、`rust-toolchain.toml`、`.gitattributes` / `.gitignore`、`.github/workflows/ci.yml`。Phase 3 已产出 `docs/acceptance.md`（264 条）与 `docs/verification.md`，待闸门 2。*
