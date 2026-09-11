# 架构设计：多 crate 分层 + 受监督任务面

本文是模板的设计依据：每条纪律配一条理由，每个决策说明为什么不选另一种。读代码之前先读这里，
改代码之前先确认要改的那条纪律还成立。

外部仓库的取证（总表、逐仓库结论、带 SHA 的 permalink）在 [references.md](references.md)，
本文只列结论。

## 目录

1. [目标与非目标](#1-目标与非目标)
2. [纪律清单](#2-纪律清单)
3. [crate 划分与依赖矩阵](#3-crate-划分与依赖矩阵)
4. [runtime 拓扑](#4-runtime-拓扑)
5. [reconcile 期望集层](#5-reconcile-期望集层)
6. [横切能力清单](#6-横切能力清单)
7. [部署布局](#7-部署布局)
8. [借鉴表](#8-借鉴表)

---

## 1. 目标与非目标

### 1.1 目标

一份能起、能停、能测的 Rust Web 服务起点，骨架里包含：

- workspace 多 crate 单向分层；
- 一个多线程 Tokio runtime + `TaskSupervisor` 顶层任务面；
- 根 `CancellationToken` 级联 + 限时收割的关停模型；
- `Arc<dyn Storage>` 门面 + sqlx 双后端 + 成对迁移 + fail-fast；
- 三段式配置加载 + `watch` 热重载；
- tracing 只在装配层初始化；
- 进程内零业务全局态；
- **可选的多 runtime 绑定**：某个任务面可以绑到独立的 Tokio runtime（独立线程预算、隔离 CPU
  密集或阻塞型工作），配置里没写就是零代价，见 §4.7。

### 1.2 非目标

- **不是框架。** 没有 `trait Service`、没有插件系统、没有宏。模板是一份纪律写在注释里的起点代码，
  复制之后改，不是依赖之后调。
- **不带业务。** 生成出来的服务只有一个示例任务面（`ticker`）和三个系统端点，别的都要你自己加。
- **不做预铺。** 不预建表、不预放 `AppState` 字段、不预抽 middleware 目录——这是 §2 A/E 组的纪律，
  模板自己先守。唯一的破例是 `api/src/extract.rs`，判据见 §6 的提取器包装一行。
- **不带任何具体部署事实。** 交叉编译目标、glibc 门禁、容器链、端口号都是项目事实，不是可复用纪律。

### 1.3 模板的用户与「第一天」

用户：从模板起一个新 Rust Web 服务的人，通常一个人，第一天就要交出一个能起、能停、能测的进程。
第一天要做的事，按顺序：

1. **生成**：`cargo generate --git <本模板仓库> --name <project-name>`，回答一个提示 `crate_prefix`
   （包名前缀）。两个名字分开：`project-name` 是项目名（仓库名、二进制名 `{{crate_name}}`、README
   标题），`crate_prefix` 是包名前缀（`<prefix>-core` / `<prefix>-storage` …）。生成后改
   `[workspace.package]` 的 `version` / `authors`。
2. **验证骨架在自己机器上成立**：`make check` 三条门禁全绿（`fmt-check` / `lint` / `test`）；
   `cargo run` 起进程，看第一条日志的构建串，`curl /v1/service/health` 得 200，Ctrl-C 看关停日志里
   每个任务的 `stopped`。
3. **把示例任务面换成自己的第一个面**：`<prefix>-worker` 里的 `ticker` 是一个只会按周期打日志与
   计数的面，按 README「怎么加一个任务面」改成真实的，或者删掉。
4. **需要库表时**走 README「怎么加一个仓储」：垂直切片八步，成对迁移 + 双后端契约测试。
5. **不碰 `[runtime.extra]`**，除非已经量出某个面在抢主 runtime 的线程（§4.2 的三个场景之一）。

本文用 `svc` 作包名前缀占位（`svc-core` / `svc-app` …）来举例；模板源码里对应的是
`{{crate_prefix}}`（包名）、`{{crate_prefix_snake}}`（Rust 路径，pre hook 派生）、`{{crate_name}}`
（二进制名，cargo-generate 内置）、`{{env_prefix}}`（环境变量前缀，pre hook 派生自 `crate_name` 大写）。

---

## 2. 纪律清单

每条都是**模板要守的纪律**。格式：纪律 → 为什么 → 代码里的落点（文件级，不写行号——行号会烂）。

### A. 依赖与门面

| #   | 纪律                                                                                                                                | 为什么                                                                      | 落点                                                      |
| --- | ----------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- | --------------------------------------------------------- |
| A1  | 依赖单向、叶子稳定：core 不依赖任何兄弟；只允许 §3.3 邻接表里的边；新增边必须写理由                                                 | 环一旦出现，编译单元与测试都无法独立；叶子稳定是整个分层能被局部替换的前提  | 根 `Cargo.toml`、§3.3                                     |
| A2  | 第三方版本与 feature 只在根 `[workspace.dependencies]` 声明一次，成员一律 `{ workspace = true }`；每个第三方 crate 旁一句「为什么」 | 版本漂移与 feature 漂移都是静默的；把理由写在依赖旁，删依赖时才知道会断什么 | 根 `Cargo.toml`                                           |
| A3  | 内部依赖边也在根表里列全；`testkit` 只允许出现在 `[dev-dependencies]`，`publish = false`                                            | 测试基建不进发布产物的依赖图                                                | 根 `Cargo.toml`、`testkit/Cargo.toml`                     |
| A4  | 窄门面：`lib.rs` / `mod.rs` 只做 `mod` 声明与显式 `pub use`，子模块默认私有，不做扁平化 re-export；每个例外出口写理由               | 出口少，换实现才不波及调用方；例外要能被审                                  | `core/src/lib.rs`、`api/src/lib.rs`、`storage/src/lib.rs` |
| A5  | 抽象生命周期，不抽象协议：共享的是「单元生命周期管理」这类骨架，不为差异大的业务强造 trait                                          | 强造的 trait 会把差异塞进参数面，比重复更贵                                 | §5                                                        |
| A6  | 低依赖、可交叉编译：TLS 走 rustls + ring，不引 aws-lc；纯 Rust 后端优先；新增依赖须论证                                             | C 构建链是交叉编译的主要负担                                                | 根 `Cargo.toml`、`core/src/tls.rs`                        |
| A7  | 叶子 crate 四条：不依赖兄弟、只 `use tracing` 不初始化、无 `tokio::spawn` 常驻任务、无 SQL                                          | 叶子承载的是「所有 crate 共享且不含常驻任务」的内容                         | `core/src/lib.rs`                                         |
| A8  | 面级 crate 的对外出口收窄到「入口函数 + 不透明句柄」，内部结构一律不转出                                                            | 外面既拼不出一次业务动作，也改不了内部状态                                  | `worker/src/lib.rs`、`worker/README.md`                   |

### B. 任务与关停

| #   | 纪律                                                                                                                                                             | 为什么                                                                            | 落点                                                |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | --------------------------------------------------- |
| B1  | 顶层任务经 `TaskSupervisor` 注册：`JoinSet` 与 `AbortHandle` 双持；first-failure——任一顶层任务退出即进程退出                                                     | 只 abort `JoinSet` 里的监控任务会让底层任务脱管；半残的进程不如死干净             | `core/src/task/supervisor.rs`、`app/src/signals.rs` |
| B2  | 收敛三模式，何处用何种写进文档注释：批量 cancel → 限时收割 → 超时 abort（任务组）；`select!` on cancel 且取消是第一分支（循环体）；`Drop` 兜底 abort（长活结构） | 绝不留 detached task，也绝不留同 key 双化身                                       | `core/src/task/mod.rs`、`supervisor.rs`             |
| B3  | 根令牌级联：一切任务用 `child_token()`；关停序列固定为「广播 `ShuttingDown` → cancel → `wait_for_shutdown(grace)` → 存储最后关」                                 | 事件先于取消，给旁路 flush 的机会；池最后关，宽限期内的收尾写入才不会变成连接错误 | `app/src/state.rs`、`app/src/signals.rs`            |
| B4  | `http` 恒为最后注册的顶层任务；TOML 侧开关关掉一个面必须 warn                                                                                                    | HTTP 开始接流量时其余面必须已就绪；「接口都在但数据不更新」是最难猜的故障         | `app/src/boot.rs`                                   |
| B5  | 启动中途失败走中止序列（取消已注册者 → 短宽限收割 → 关库）并返回**原始**错误                                                                                     | 半装配的进程对外服务只会把启动问题拖成运行时问题                                  | `app/src/boot.rs`                                   |
| B6  | 空 supervisor 正常退出，不判 first-failure                                                                                                                       | `next_exit` 对空集立即返回 `Empty`，不特判会把「没任务」误判成故障                | `app/src/signals.rs`                                |
| B7  | 关停预算分层：内层收割 < 外层宽限，留余量；bind 失败原样上抛、不重试                                                                                             | 内外取同值会让内层吃满、外层没时间收尾                                            | `app/src/boot.rs`、`api/src/lib.rs`                 |
| B8  | 旁路纪律：旁路失败只记日志与计数，不向上传播、不重试、不动内存态；超时与失败**分开计数**                                                                         | 主路径不该因旁路停摆；两个数字要查的方向相反                                      | `worker/README.md`                                  |
| B9  | 时长走单调钟（`Instant`），墙钟只用于展示；坏钟不 panic（`now_ms` 饱和到 `0` / `i64::MAX`）                                                                      | panic 会被管理器当成单元死亡反复重建，一次对时能引发重启风暴                      | `core/src/util/time.rs`                             |
| B10 | 进程内无业务全局态：`Metrics` / `EventBus` / `ConfigHandle` / `Storage` 全部由装配层构造并注入                                                                   | 测试并行化（去 `--test-threads=1`）的前提；一个进程里能跑两套装配                 | `app/src/state.rs`、`core/src/metrics.rs`           |

### C. 配置

| #   | 纪律                                                                                                                                                                                  | 为什么                                                                             | 落点                                           |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ---------------------------------------------- |
| C1  | 一条管线，启动与热重载共用：读文件（缺文件落盘内嵌模板）→ `${env.NAME:default}` 展开 → 反序列化（`serde_path_to_error` 定位）→ `resolve_paths` → `complete` → `validate` → `sanitize` | 两条管线迟早漂移                                                                   | `core/src/config/app.rs`                       |
| C2  | `validate` 只拦「继续跑必然失败」的取值；其余越界一律钳位 + warn                                                                                                                      | 无人值守场景下拒绝启动的代价远大于按保守值跑                                       | `core/src/config/app.rs`、`clamp.rs`           |
| C3  | 每段 `#[serde(default, deny_unknown_fields)]`                                                                                                                                         | 拼错键即错误，缺段等价默认，零配置可起                                             | `core/src/config/app.rs`、`cfg_http.rs`        |
| C4  | 写端 `ConfigStore` 只在装配层；读端 `ConfigHandle` 廉价 Clone 随处注入；可热字段每 tick 现读                                                                                          | 写端流散就有人随手改配置                                                           | `core/src/config/store.rs`、`api/src/state.rs` |
| C5  | 热重载白名单三档：可热 / 半热（启动闸）/ 不可热；不可热段 reload 时**回滚为运行值**并列入 `requires_restart`                                                                          | watch 里的配置必须永远等于「生效中」的配置，否则读端看到新端口、实际仍 bind 旧端口 | `core/src/config/store.rs`、`cfg_http.rs`      |
| C6  | 重载失败保留 last-good，绝不半套用                                                                                                                                                    | 半套用的配置比旧配置更难排查                                                       | `app/src/signals.rs`                           |
| C7  | 内嵌模板 `default.toml` 取值与 `Default` 逐字段一致，有测试钉住                                                                                                                       | 模板是文档，`Default` 是行为，二者不同即撒谎                                       | `core/src/config/app.rs`、`default.toml`       |
| C8  | 敏感信息：`password_env > password_file > password`；手写 `Debug` 渲染 `***`                                                                                                          | 密码不能随错误信息进日志                                                           | `core/src/config/cfg_storage.rs`               |

冷段白名单**按后缀通判**而不是按面登记：`classify` 认 `.runtime` / `.enabled` 后缀，
`COLD_PREFIXES` 只留整段不可热的三个前缀（`http.` / `storage.` / `runtime.`）。按面登记的方案在
漏登记时是**静默**失败——热重载假报「已生效」，而实际那个字段在 spawn 那一刻就定死了。

> 已知缺口：`store.rs::reload` 的回滚块仍按面硬编码（`fresh.<面>.runtime = old...`），加一个面时
> 漏改同样静默。加面时按 README 的接线表补一条回滚测试。

### D. 存储

| #   | 纪律                                                                                                                                            | 为什么                                                                              | 落点                                               |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | -------------------------------------------------- |
| D1  | `Arc<dyn Storage>` 门面；公共 API 不出现 sqlx 类型；上层拿不到连接池                                                                            | 拿不到池就不可能绕过门面发 SQL；换后端不波及调用方                                  | `storage/src/repo.rs`、`storage/README.md`         |
| D2  | `init_storage` 唯一入口：连接 → 迁移 → 自检 → 返回，任一步失败 fail-fast；自检失败先显式关池再上抛                                              | 带着不可用的存储跑起来，故障推迟到首次写入才暴露；池的后台任务不随 `Arc` 析构立即停 | `storage/src/backend/mod.rs`                       |
| D3  | 存储层不持有常驻任务；维护动作由上层调度                                                                                                        | 常驻任务只能在装配层被监管                                                          | `storage/README.md`、`storage/src/repo.rs`         |
| D4  | SQLite 双池：writer `max_connections=1`（物理单写者）+ 只读池；两池 `acquire_timeout` 显式设短。PG 单池、不自动建库、连接选项用离散字段不用 DSN | 排队本身会成为故障放大器；DSN 会让密码随错误信息泄露                                | `storage/src/backend/sqlite.rs`、`postgres.rs`     |
| D5  | 迁移成对新增、已发布不可改、按真实消费者取号；`build.rs` 对迁移目录 `rerun-if-changed`，不可删                                                  | `sqlx::migrate!` 对新增文件静默不生效                                               | `storage/migrations/README.md`、`storage/build.rs` |
| D6  | 垂直切片八步扩展；绝不单后端落地；不预建表；不提供假成功实现                                                                                    | 差异在切换后端时才暴露，那时修复最贵                                                | `storage/README.md`                                |
| D7  | 错误按语义归一化（`NotFound` / `Conflict` / `UniqueViolation` / `VersionConflict`），不按表拆                                                   | 按表拆会让每加一张表就改一次公共 API                                                | `storage/src/error.rs`                             |
| D8  | 启动失败对常见迁移错误给可执行修复指引（`VersionMismatch` / `VersionMissing` / `Dirty`）                                                        | 原始信息只说「不匹配」，看到的人会去手改 `_sqlx_migrations`                         | `storage/src/backend/mod.rs`                       |
| D9  | `Storage: Debug`，实现只输出后端名，绝不打印连接串或凭据                                                                                        | 持有 `Arc<dyn Storage>` 的结构体要能 `derive(Debug)`                                | `storage/src/repo.rs`、`backend/sqlite.rs`         |

### E. HTTP

| #   | 纪律                                                                                                                                        | 为什么                                                                                                      | 落点                                      |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ----------------------------------------- |
| E1  | 公开门面只有 `build_router` / `bind` / `serve` / `AppState`；handler / error / response 模块全私有                                          | 端点只能经 HTTP 消费；响应契约由单测钉，不构成代码级 API                                                    | `api/src/lib.rs`、`api/README.md`         |
| E2  | 每组 handler `pub(crate) fn router() -> Router<AppState>`，`build_router` 里 merge 后 `nest("/v1")`；加一组端点 = 加一个子模块 + 一行 merge | 既有组不动                                                                                                  | `api/src/handler/mod.rs`、`api/README.md` |
| E3  | 只有一个 layer 就不建 middleware 目录                                                                                                       | 提前抽象                                                                                                    | `api/src/lib.rs`                          |
| E4  | `/v1/**` 未命中回 JSON 信封 404，回显 method + path；`nest` 的尾斜杠缺口要显式补路由                                                        | 404 只说「没找到」等于没说；`/v1/` 两头都不中会掉出嵌套                                                     | `api/src/lib.rs`                          |
| E5  | 错误到 HTTP 状态码的映射集中在一处（`storage_semantics`，两个 `From` 入口共用）；`From<AppError>` 沿 `source()` 链还原存储语义              | 两边跑偏是迟早的事；只看顶层类型会让经 `AppResult` 升格过的 404 / 409 静默变 500                            | `api/src/error.rs`                        |
| E6  | `AppState` 字段准入：只在出现真实消费者时添加；读写分离（api 只持配置读端）                                                                 | 写端被随手拿来改配置                                                                                        | `api/src/state.rs`                        |
| E7  | HTTP 面不自持任务：返回给 app 注册；`axum::serve(...).with_graceful_shutdown(token.cancelled_owned())`                                      | 任务只能在装配层被监管                                                                                      | `api/src/lib.rs`                          |
| E8  | 不初始化 tracing、不建常驻任务、不落库细节                                                                                                  | 与 A7 同源                                                                                                  | `api/README.md`                           |
| E9  | 三条响应契约由类型钉住：成功带数据回裸 JSON、成功无数据与所有错误回 `{status, code, description}` 同形信封                                  | 约定会被改坏，类型不会；`GenericResponse` 由 `response.rs` 与 `error.rs` 共用，两个信封不可能长歪成两种形状 | `api/src/response.rs`、`api/src/error.rs` |

系统端点整组挂在 `/v1/service/` 下（`health` / `ready` / `info`），**不在版本面之外另开无版本路径**。
留一片 `/healthz` 之类的裸路径等于给自己开第二套契约：第二个未命中兜底、第二处响应形状，而这正是
E1/E4 想收拢的东西。「编排器按固定路径探活」不构成反对理由——那个路径本来就是编排器配置里的一个
字符串，跟着版本走没有额外成本。

### F. 测试

| #   | 纪律                                                                                            | 为什么                                                | 落点                                                |
| --- | ----------------------------------------------------------------------------------------------- | ----------------------------------------------------- | --------------------------------------------------- |
| F1  | 黑盒 oneshot 直打 `Router`，不 bind 端口；契约单测直接在 `IntoResponse` 输出上断言              | 真路由、真提取器、真序列化都在被测面里，且无端口冲突  | `api/tests/common/mod.rs`、`api/tests/system.rs`    |
| F2  | 装配级测试走真 TCP + 真配置文件 + 临时目录；端口按 pid 派生；**不为测试改生产 bind 逻辑**       | 证的是「bind 出来的 socket 在服务」而不是路由表长得对 | `app/src/boot.rs`（测试模块）、`testkit/src/lib.rs` |
| F3  | 双后端契约矩阵：同一组断言跑两次；PG 由环境变量门控，**声明了却连不上要红而不跳**               | 静默跳过让「PG 没跑过」和「PG 全绿」一模一样          | `storage/tests/`、`storage/README.md`               |
| F4  | 决策纯函数化，任务类逻辑不起运行时可测；驱动循环用 `start_paused` 模拟时钟                      | 「等一等再断言」是任务类测试不稳定的主要来源          | `reconcile/src/plan.rs`、`reconcile/src/driver.rs`  |
| F5  | 每条关键不变量至少有一条自动化证据（例：http 最后注册由 bind 失败时的 `supervisor.len()` 钉住） | 注释会被改坏，测试不会                                | `app/src/boot.rs`（测试模块）                       |
| F6  | 测试不装全局 subscriber；日志断言用私有 subscriber 捕获                                         | 全局 subscriber 让测试串数                            | `testkit/src/lib.rs`                                |

### G. 注释与文档

| #   | 纪律                                                                                                     | 为什么                   | 落点                                  |
| --- | -------------------------------------------------------------------------------------------------------- | ------------------------ | ------------------------------------- |
| G1  | 注释写「为什么」不写「是什么」；每个非显然决策旁边都有理由；密度看齐根 `Cargo.toml` 与 `app/src/boot.rs` | 「是什么」代码自己会说   | 根 `Cargo.toml`、`app/src/boot.rs`    |
| G2  | crate 级 README 四段：边界 / 目录 / 关键决策 / 测试形态                                                  | 读者先知道边界再看代码   | 各 crate 的 `README.md`               |
| G3  | 冻结语义显式标注（「既定不变量」「冻结语义」「不得修正」），改动前先读懂                                 | 让「这条能不能改」不靠猜 | `app/src/boot.rs`、`api/src/state.rs` |
| G4  | 设计文档先摆事实再给方案，每条决策带理由                                                                 | 本文自身遵守             | 本文                                  |

### H. 工程链

| #   | 纪律                                                                                                           | 为什么                                                                                                                                                                            | 落点                                             |
| --- | -------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| H1  | 生成结果的门禁三条：`fmt --check`、`clippy --workspace --all-targets -D warnings`、`test --workspace --locked` | `--locked` 让 CI 与本地是同一张依赖图                                                                                                                                             | `Makefile.project`                               |
| H2  | release profile：`lto` / `codegen-units=1` / `strip` / `panic=abort`，另备 `release-size`                      | 边缘与容器都在乎体积                                                                                                                                                              | 根 `Cargo.toml`                                  |
| H3  | 构建串经 `option_env!` 编译期注入；第一条日志固定打构建串                                                      | 现场排查只看日志开头就知道跑的哪个版本                                                                                                                                            | `core/src/util/build_info.rs`、`app/src/boot.rs` |
| H4  | 版本只在 `workspace.package.version` 一处                                                                      | 多处版本必漂移                                                                                                                                                                    | 根 `Cargo.toml`、`core/src/lib.rs`               |
| H5  | **模板仓库自己**的门禁是五条：三条之前多 `fmt-portable` 与 `fmt-matrix`                                        | 模板源码里是 `{{...}}`，自己没法 rustfmt；而 rustfmt 的排版取决于**替换之后**的行宽与排序，「对某一组名字 fmt-clean」不蕴含「对任意名字都是」。生成结果里没有占位符，用不上这两条 | `Makefile`、`scripts/check-fmt-portability.py`   |

---

## 3. crate 划分与依赖矩阵

### 3.1 分层图

```
 层4 装配    ┌───────────────────────────────────────────────┐
            │  svc-app (bin)  ──► core, storage, worker, api │   reconcile 不在默认依赖里
            └───────────────────────────────────────────────┘
 层3 展示    ┌───────────┐
            │  svc-api  │ ──► core, storage
            └───────────┘
 层2 面      ┌────────────┐      ┌───────────────┐
            │ svc-worker │      │ svc-reconcile │ ◄── 可选：默认无人依赖，但随 workspace 一起构建与测试
            └────────────┘      └───────────────┘
 层1 资源    ┌─────────────┐
            │ svc-storage │ ──► core
            └─────────────┘
 层0 内核    ┌───────────────────────────────────────────────┐
            │                svc-core（叶子）                 │
            └───────────────────────────────────────────────┘
 测试        svc-testkit ──► core, storage      只允许出现在各 crate 的 [dev-dependencies]
```

### 3.2 直接依赖邻接表（除此之外的边一律非法）

| crate           | 直接依赖                   | 备注                                                        |
| --------------- | -------------------------- | ----------------------------------------------------------- |
| `svc-app`       | core、storage、worker、api | 装配层；`reconcile` 只在某个面真的用它时才加边              |
| `svc-api`       | core、storage              | **不依赖 worker**：接口面读运行数据只能经 core 里的注入类型 |
| `svc-worker`    | core                       | 示例面不落库。真实的面要存储就加 `storage` 边，在评审说明   |
| `svc-reconcile` | core                       | 只提供生命周期骨架，无常驻任务、无 SQL                      |
| `svc-storage`   | core                       |                                                             |
| `svc-core`      | —（叶子）                  |                                                             |
| `svc-testkit`   | core、storage              | 仅 dev                                                      |

**明令禁止的边**：

- `core → *`：叶子。
- `storage → api / worker / app`：资源层不知道谁在用它。
- `api → worker`、`worker → api`：两个面的衔接介质只能是 core 里的共享态与事件。模板**不带任何
  例外反向边**；需要时走「core 定 trait / 面实现 / app 注入」三角。
- `* → app`：装配层是根，没有人依赖它。
- `testkit` 出现在任何 `[dependencies]` 里。

### 3.3 每个 crate 的骨架内容

```text
svc-core/src/
  lib.rs            模块声明 + 唯一例外出口 pub use error::{AppError, AppResult}；SERVICE_NAME
  error.rs          AppError（Io / Config / Storage(anyhow) / Internal(anyhow)）
  config/
    mod.rs          窄门面：load / load_or_init / AppConfig / ConfigStore / ConfigHandle / ReloadReport
    app.rs          AppConfig + 七步管线
    default.toml    内嵌配置模板（缺文件时落盘的那一份；与 Default 逐字段一致，有测试钉住）
    cfg_http.rs     [http]    host / port / runtime（不可热）
    cfg_storage.rs  [storage] backend / sqlite / postgres（不可热；密码三级取值）
    cfg_runtime.rs  [runtime] worker_threads / max_blocking_threads / extra.<name>（不可热）
    cfg_ticker.rs   [ticker]  interval_ms（可热）/ enabled（半热）/ runtime（不可热）——示例面的段
    clamp.rs        越界钳位 + warn（C2）
    env_expand.rs   ${env.NAME:default} 手写解析器（零依赖）
    path_util.rs    配置里的相对路径以安装根为基准（§7）
    store.rs        ConfigStore（watch 写端 + reload + 冷段回滚）/ ConfigHandle
    error.rs        ConfigError（教学式；含 Rust 格式串的转义花括号，故列进 cargo-generate 的 exclude）
  events.rs         EventBus / EventStream / AppEvent { DesiredSetChanged{plane}, ConfigReloaded{generation}, ShuttingDown }
  metrics.rs        Metrics { ticker: TickerStats } + LastError（只随真实消费者长字段）
  task/
    mod.rs          三模式文档注释 + pub use supervisor::*
    supervisor.rs   TaskSupervisor
  util/
    mod.rs          子模块声明
    build_info.rs   BuildInfo（option_env! 注入 APP_VERSION / APP_BUILD / APP_COMMIT_SHA / APP_DESCRIPTION）
    paths.rs        install_root()：可执行文件所在目录（§7）
    time.rs         TimestampMs / now_ms
  tls.rs            ensure_crypto_provider（rustls + ring）

svc-storage/
  build.rs          迁移目录 rerun-if-changed（不可删）
  migrations/{sqlite,postgres}/README.md   目录先存在、零迁移
  src/
    lib.rs          pub use backend::init_storage; error::*; repo::Storage
    error.rs        StorageError（Init / Db / Migrate / NotFound / Conflict / UniqueViolation / VersionConflict）+ From<StorageError> for AppError
    repo.rs         trait Storage { backend_name / health / run_maintenance / close }——仓储访问器按垂直切片加
    model.rs        StorageHealth
    backend/mod.rs  init_storage + log_startup_hint
    backend/sqlite.rs   双池 + PRAGMA + 关停收紧权限
    backend/postgres.rs 单池 + sslmode + 密码解析
  tests/            bootstrap（门面形状）+ sqlite_bootstrap / postgres_bootstrap（双后端启动契约）+ support/

svc-worker/src/
  lib.rs            pub use ticker::run as ticker  （窄门面：一个入口）
  ticker.rs         fn run(config, metrics, cancel) -> impl Future<Output = ()> + Send + 'static
                    ——select! 取消第一分支；interval 每 tick 从 ConfigHandle 现读（可热）；计数进 metrics

svc-api/src/
  lib.rs            build_router / bind / serve；两个内联 layer：TraceLayer + TimeoutLayer
  state.rs          AppState { storage, config: ConfigHandle, events, metrics }
  error.rs          HttpError { Rejected(StatusCode, String) / NotFound / Conflict / ServiceUnavailable / Internal } + storage_semantics + From<StorageError> + From<AppError>（沿 source 链还原存储语义）
  response.rs       ApiResponse { Ok（成功信封）/ Data（裸 JSON） } + GenericResponse（与 error.rs 共用）
  extract.rs        Json / Query / Path 包装：拒绝也走同形信封（§6）
  handler/mod.rs    只声明子模块
  handler/system.rs 整组挂 /v1/service/：GET health（进程活着即 200）/ GET ready（storage.health() 通过才 200，否则 503）/ GET info（构建串 + 配置代数）
  tests/common/mod.rs + tests/system.rs   oneshot 黑盒

svc-app/src/
  main.rs           解析 CLI → tracing → 同步加载配置 → Runtimes::build → block_on(boot_strap) → Runtimes::shutdown → 退出码
  cli.rs            --version / --config（手写）；locate_config 一次返回 (安装根, 配置路径)
  rt.rs             Runtimes / Executors（§4）
  boot.rs           boot_strap 六步 + register_runtime_tasks（http 最后；开关关掉 warn）+ abort_boot
  signals.rs        monitor 四路 select + shutdown_runtime
  state.rs          RuntimeState { config: ConfigStore, events, storage, metrics, executors, shutdown }

svc-testkit/src/
  lib.rs            TempStorage（临时目录 + SQLite 文件库）/ LogCapture（私有 subscriber 的 MakeWriter）/ test_port(offset)

svc-reconcile/src/   见 §5

（模板仓库级，不进生成结果）
cargo-generate.toml   [placeholders] crate_prefix；[hooks] pre + post；[template] ignore / exclude
hooks/pre.rhai        派生 crate_prefix_snake（`-`→`_`）与 env_prefix（crate_name 大写）
hooks/post.rhai       Makefile.project / README.project.md 改回正名；hooks/ 自删（它不能进 ignore：ignore 先于 post hook 生效）
Makefile              gen / check（五条门禁）/ verify（cargo generate --test）/ lock
.github/workflows/    ci.yml：check job（带 PostgreSQL service）+ verify job
scripts/              check-fmt-portability.py（静态扫描）、template-sync.py（生成目录的格式化结果反写回模板源码）
Makefile.project      生成结果拿到的 Makefile（由 post hook 改名）
README.project.md     生成结果拿到的 README（由 post hook 改名）
```

---

## 4. runtime 拓扑

### 4.1 默认路径：一个多线程 runtime + `TaskSupervisor`

三条契约撑起整个模型：

1. **任务面返回 future，app 决定 spawn 到哪。**

   ```rust
   // svc-worker/src/ticker.rs —— 面只描述「跑什么」，不决定「在哪跑」
   pub fn run(config: ConfigHandle, metrics: Arc<Metrics>, cancel: CancellationToken)
       -> impl Future<Output = ()> + Send + 'static
   ```

   理由：「跑在哪个 runtime」是装配层的事；面里写死 `tokio::spawn` 就等于把绑定写死在面里。
   单 runtime 时两种写法等价，代价是 app 多一个 `.spawn(...)`；收益是多 runtime 不需要改任何面的
   代码。面内部再 `tokio::spawn` 子任务时，自然落在被 spawn 到的那个 runtime 上（tokio 以当前上下文
   为准），所以二级任务也跟着面走，不需要额外传 `Handle`。

2. **HTTP 面拆成 `bind`（同步、可失败）+ `serve`（future）。** bind 失败原样上抛让启动中止
   （有测试钉住）。bind 用 `std::net::TcpListener`（同步），在 `serve` 的 future 里 `set_nonblocking`
   - `tokio::net::TcpListener::from_std`。原因见 §4.4 第 2 条：tokio 的 IO 资源在**创建时**注册到当前
     runtime 的驱动，HTTP 面若绑到附加 runtime，socket 必须在那个 runtime 里完成注册。

3. **`register_runtime_tasks` 的每条注册先解析 executor**：

   ```rust
   // svc-app/src/boot.rs
   let cfg = state.config.current();
   if cfg.ticker.enabled {
       let exec = state.executors.resolve(cfg.ticker.runtime.as_deref())?;   // None → 主 runtime
       supervisor.register("ticker", exec.spawn(svc_worker::ticker(
           state.config.handle(), state.metrics.clone(), state.shutdown.child_token(),
       )));
   } else {
       tracing::warn!("ticker disabled by [ticker].enabled, nothing will tick");   // B4：关掉必须喊
   }
   // http 最后（既定不变量）；bind 同步 fail-fast，serve 在目标 runtime 上完成 socket 注册
   let listener = svc_api::bind(&cfg.http)?;
   let exec = state.executors.resolve(cfg.http.runtime.as_deref())?;
   supervisor.register("http", exec.spawn(svc_api::serve(api_state, listener, state.shutdown.child_token())));
   ```

### 4.2 多 runtime：什么场景才值得单开

先说不值得的：**I/O 密集的面（HTTP、数据库、消息队列、定时轮询）一律不值得。** tokio 的一个多线程
runtime 就是为它们设计的，再开一个只会多一组空转线程，并把「共享一个 IO 驱动」变成「两个驱动各醒
各的」。「逻辑上分开」不是理由，crate 边界已经分开了。

值得的三种情况，每种都要**先量出来**再动手：

| 场景                  | 症状                                                                                                        | 为什么单开 runtime 有效                 | 更便宜的替代（先试）                                     |
| --------------------- | ----------------------------------------------------------------------------------------------------------- | --------------------------------------- | -------------------------------------------------------- |
| CPU 密集的 async 循环 | HTTP 延迟随该面负载抖动；tokio 的协作式调度无法抢占一个不 `await` 的任务                                    | 独立 worker 池，坏邻居只饿死自己        | 把计算段包进 `spawn_blocking`，或每 N 次迭代 `yield_now` |
| 阻塞型 FFI / 同步 SDK | `spawn_blocking` 用量打满 `max_blocking_threads`，其他面的 `spawn_blocking`（如 `sqlx` 的 SQLite 驱动）排队 | blocking 池预算按 runtime 分，互不抢    | 调大主 runtime 的 `max_blocking_threads`                 |
| 延迟敏感面需要隔离    | 某个面的 P99 有 SLO，不能受批处理面拖累                                                                     | 独立线程预算 = 独立的调度队列与 IO 驱动 | 给批处理面加并发上限（`Semaphore`）                      |

这三条也是 README「怎么给一个面单开 runtime」一节的准入清单：**没量过就不开**。

**附加 runtime 只有多线程一种形态**，`worker_threads = 1` 即单线程隔离。不提供
`flavor = "current_thread"`：`new_current_thread()` 建出的 runtime 要有人 `block_on` 才轮询任务，而
附加 runtime 在本设计里只当 spawn 目标，没有任何线程对它 `block_on`——`Handle::spawn` 进去的任务
永不执行，`await` 其 `JoinHandle` 直接挂死。要让它可用需要额外一条专用 OS 驱动线程和独立关停路径；
单 worker 的多线程 runtime 已给到同等隔离且自带驱动。`ExtraRuntimeConfig` 的 `deny_unknown_fields`
会让残留 `flavor = ...` 的老配置当场报错，而不是静默挂死。

### 4.3 怎么表达「某个任务面绑定到哪个 runtime」

三段配置 + 一个装配层类型，**零新依赖、零新 trait**：

```toml
# <安装根>/config/<服务名>.toml
[runtime]                       # 整段不可热：runtime 在 block_on 之前就建好了
worker_threads = 0              # 0 = 跟随 CPU 数（tokio 默认）
max_blocking_threads = 512      # tokio 默认

[runtime.extra.compute]         # 可选：每个子表一个附加 runtime。缺省没有子表 = 单 runtime
worker_threads = 2              # 写 1 就是单线程隔离
max_blocking_threads = 4

[ticker]
interval_ms = 1000              # 可热
runtime = "compute"             # 不可热：绑定在 spawn 那一刻定死。缺省 = 主 runtime
```

```rust
// svc-app/src/rt.rs —— 装配层，不进 core（造 runtime 不是「所有 crate 共享」的内容）
pub struct Runtimes {
    main: tokio::runtime::Runtime,
    /// 按配置顺序；关停时逆序
    extra: Vec<(String, tokio::runtime::Runtime)>,
}
impl Runtimes {
    /// 每个 [runtime.extra.<name>] 一个；线程名 "<name>-N"，日志与 `ps -M` 里能认出来
    pub fn build(cfg: &RuntimeConfig) -> std::io::Result<Self>;
    pub fn executors(&self) -> Executors;
    pub fn block_on<F: Future>(&self, f: F) -> F::Output;
    /// 同步；只能在 block_on 返回之后调（§4.5）
    pub fn shutdown(self, grace: Duration);
}

/// Handle 集合，Clone 廉价；进 RuntimeState 供 register_runtime_tasks 解析
#[derive(Clone)]
pub struct Executors { main: Handle, extra: HashMap<String, Handle> }
impl Executors {
    /// None → 主 runtime。Some(未配置的名字) → 启动错误。
    /// 不静默回落到主 runtime：回落会把「配置写错」藏成「性能不达预期」，那是最难查的一类故障
    pub fn resolve(&self, name: Option<&str>) -> AppResult<&Handle>;
}
```

绑定放在**每个面的配置段**（`[ticker].runtime`）而不是集中一张表，理由：面的开关、周期、绑定都在
同一段里，运维改一个面只看一段；「给一个面单开 runtime」变成两处 TOML、零行代码。绑定字段与
`[runtime]` 段一样归入不可热白名单，`reload` 时回滚为运行值（C5）。

**`TaskSupervisor` 全局一个，放主 runtime。** 理由：

- `JoinHandle` / `AbortHandle` 不绑定 runtime，`register` 收哪个 runtime 的句柄都一样；`JoinSet`
  里的监控任务跑在主 runtime 上，只是在 `await` 一个别处的句柄。
- first-failure 是**进程级**语义：任何一个面退出都要让进程退出。每 runtime 一个 supervisor 就得再
  加一层聚合，除了多一层没有任何收益。
- `Drop` 兜底 abort 跨 runtime 同样有效。

### 4.4 跨 runtime：三条规则

1. **通道与令牌天然跨 runtime，照旧用。** `tokio::sync`（broadcast / watch / mpsc / oneshot）与
   `tokio_util::sync::CancellationToken` 内部只有原子量与 waker，不持有 runtime。根令牌
   `child_token()` 的级联不需要为多 runtime 做任何事；`EventBus` 与 `ConfigHandle` 同理。
2. **IO 资源随创建时的 runtime 注册。** `TcpListener` / `TcpStream` / sqlx 连接池在哪个 runtime 的
   上下文里创建，就由哪个 runtime 的驱动唤醒（`from_std` 在没有 IO 驱动的上下文里直接 panic：
   tokio 1.53.1 `src/net/tcp/listener.rs:234-241`）；从别的 runtime 去 poll 它是可以的，但**创建它
   的 runtime 必须活得更久**。由此两条落地规则：
   - 共享资源（存储池、事件总线）一律在主 runtime 里创建，主 runtime 最后关（§4.5）；
   - 面私有的 IO 在面的 future 里创建——HTTP 的 socket 因此走「同步 bind + future 内 `from_std`」。
3. **阻塞与预算按 runtime 分。** `spawn_blocking` 落在**当前** runtime 的 blocking 池；不得在任何
   runtime 线程上调 `Handle::block_on`（tokio 直接 panic）；`block_in_place(|| handle.block_on(..))`
   只在多线程 runtime 的 worker 线程上合法，模板不用它做跨 runtime 调用；`tokio::time::pause` 只
   作用于当前 runtime，测试要注意。

这三条写在 `rt.rs` 的模块注释里，不靠读者记。

### 4.5 关停顺序（跨 runtime）

```
monitor 退出（信号 / first-failure）
  → events.publish(ShuttingDown)                    ← 先于取消，给旁路 flush 的机会（B3）
  → shutdown.cancel()                                ← 根令牌级联，不分 runtime
  → supervisor.wait_for_shutdown(SHUTDOWN_GRACE)     ← 收割全部顶层任务，不管它们在哪个 runtime；超时逐个 abort
  → storage.close()                                  ← 池最后关；池活在主 runtime 上
  ── block_on 返回，回到同步的 main ──
  → Runtimes::shutdown(RUNTIME_GRACE)
      for (name, rt) in extra.rev():  rt.shutdown_timeout(RUNTIME_GRACE)
      main.shutdown_timeout(RUNTIME_GRACE)            ← 主 runtime 最后；此时理应已无任务
  → 退出码
```

两个「为什么」：

- **为什么附加 runtime 在 `block_on` 之后同步关。** 在 async 上下文里 drop 一个 `Runtime` 会 panic
  （tokio 1.53.1 `src/runtime/blocking/shutdown.rs:52`："Cannot drop a runtime in a context where
  blocking is not allowed"）；`shutdown_background` 是文档给出的 async 内替代，但它不等任务收尾，
  顺序就看不出来了。`shutdown_timeout` 的语义是「最多等 `duration` 让全部已 spawn 的工作停下」。
  同步关是唯一既确定又可观测的位置。三个生产项目踩的是同一条约束，三种绕法，见 §8.2。
- **为什么到这一步理应已无任务，还要给 `RUNTIME_GRACE`。** `wait_for_shutdown` 收的是**顶层**任务；
  面内部 `tokio::spawn` 出去又没有随 cancel 收敛的子任务（这是 bug，但模板要兜住），会在
  `shutdown_timeout` 到期时被 abort 并留下一条 warn——这是「无 detached task」验收的最后一道网。

### 4.6 线程预算配置化

| 配置键                                      | 映射到                                            | 缺省                          | 说明                                                  |
| ------------------------------------------- | ------------------------------------------------- | ----------------------------- | ----------------------------------------------------- |
| `runtime.worker_threads`                    | `Builder::worker_threads`                         | 0 → 不调用（tokio 取 CPU 数） | `sanitize` 钳位到 `[1, 1024]`                         |
| `runtime.max_blocking_threads`              | `Builder::max_blocking_threads`                   | 512                           | 同上                                                  |
| `runtime.extra.<name>.worker_threads`       | `new_multi_thread` + `Builder::worker_threads`    | 1                             | 附加 runtime 的缺省故意小：它是隔离手段，不是扩容手段 |
| `runtime.extra.<name>.max_blocking_threads` | 同上                                              | 16                            | 同上                                                  |
| （所有 runtime）                            | `thread_name_fn` = `"<name>-<seq>"`、`enable_all` |                               | 主 runtime 名 `main`                                  |

### 4.7 证明：不用多 runtime 的服务不为它付复杂度

逐项列代价，配置里没有 `[runtime.extra]` 子表时：

| 维度     | 单 runtime 服务付出的                                       | 说明                                                                                              |
| -------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| 线程     | 0 个额外线程                                                | `extra` 为空 vec；`Runtimes::build` 只建一个 runtime                                              |
| 依赖     | 0 个新 crate                                                | 全部是 `tokio` 已有 API                                                                           |
| 面的代码 | 0 行                                                        | 面返回 future 的契约与 runtime 数量无关                                                           |
| 配置     | 0 行必填                                                    | `[runtime]` 与 `<面>.runtime` 全部有缺省；`deny_unknown_fields` 之下不写就是不存在                |
| app 代码 | `rt.rs` 约 80 行 + `main.rs` 比 `#[tokio::main]` 多约 12 行 | 这 12 行的主要动机是把主 runtime 的线程预算变成配置项（§4.6），即便没有多 runtime 也要付          |
| 关停     | 多一段空循环                                                | `extra.rev()` 为空；主 runtime 的 `shutdown_timeout` 在 `#[tokio::main]` 下本来也会在 drop 时发生 |
| 测试     | 0 条额外前置                                                | 多 runtime 的测试在 app 里自建 `Runtimes`，不影响其他测试                                         |

**为什么不做成 cargo feature。** 这段逻辑不到 100 行，全部是 tokio 的稳定 API；用 feature 门控的
代价是「默认构建里不编译、不测试」，而 `cargo test --workspace --locked` 是硬门禁——被门控掉的代码
会在没人开 feature 的几个月里悄悄坏掉。配置门控让它永远在编译、永远在测试，而运行期代价如上表为零。

---

## 5. reconcile 期望集层

`svc-reconcile` 解决一个通用问题：**一组由配置或库表决定的长活单元，配置变了就关旧建新、死了就
退避重建、超限就放弃。** 每租户 worker、每设备会话、每队列消费者都是这个形状。

四个文件：

| 文件         | 内容                                                                                                                                                                       |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `unit.rs`    | 契约：`UnitSpec` / `UnitFactory` / `UnitSource` / `UnitCtx`（只有 `cancel` 与 `progress`，业务依赖由工厂持有）                                                             |
| `plan.rs`    | 纯函数决策：当前集 + 期望集 → 关哪些、建哪些、退避多久。用例矩阵在同文件                                                                                                   |
| `manager.rs` | 执行与记账：按计划关停与拉起，记录代数、失败次数、退避截止                                                                                                                 |
| `driver.rs`  | 驱动循环：事件唤醒 + 兜底轮询；`drive` 收一个 `wake: impl FnMut(&AppEvent) -> bool` 谓词（常规传 `wake_on_plane(<面名>)`），防自激——谓词为假的事件既不唤醒也不重置兜底期限 |

**为什么是独立 crate 而不是 `core::task` 的第二层。** 它约 1800 行，只有部分用户需要；放进叶子
crate 意味着每个服务都编译它。做成独立 workspace 成员之后：默认不进二进制（`svc-app` 不依赖它，
`cargo tree -p svc-app` 里没有），但 `cargo test --workspace` 仍然测它——不会在没人用的几个月里
悄悄坏掉。需要时加一条 `<面> → reconcile` 的边即可。

示例面 `ticker` **不用**它——骨架片要最小。

它的测试纪律（纯函数决策、`start_paused` 模拟时钟、防自激）是 F4 那条纪律的完整示范。

---

## 6. 横切能力清单

三档：**内置**（默认路径就有）/ **可选**（在模板里、默认不启用）/ **不进**。

| 能力                                                              | 处置                               | 形态                                                                                           | 理由                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------- | ---------------------------------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| config：七步管线                                                  | 内置                               | `svc-core::config`，`load` / `load_or_init`                                                    | C1–C3                                                                                                                                                                                                                                 |
| config：`${env.NAME:default}` 占位符                              | 内置                               | 手写解析器，零依赖                                                                             | PG 密码等口子统一。定界符用 `${ }` 而非 `{{ }}`：后者是 Liquid 语法，模板经 cargo-generate 展开时会被当成占位符解析——留着就得把配置模板、解析器测试、README 全部列进 `exclude` 或包 `{% raw %}`，任何新文件提到这个语法都会在生成时炸 |
| config：`ConfigStore` / `ConfigHandle` + `watch`                  | 内置                               | 写端只在 app                                                                                   | C4                                                                                                                                                                                                                                    |
| config：SIGHUP 热重载 + `ReloadReport` + 冷段回滚                 | 内置                               | 冷段白名单：`http.*`、`storage.*`、`runtime.*`，外加 `.runtime` / `.enabled` 后缀通判          | C5–C6；模板必须演示可热字段（`ticker.interval_ms`）否则机制是死的                                                                                                                                                                     |
| config：`POST /v1/service/config/reload`                          | 不进                               |                                                                                                | SIGHUP 已覆盖；多一个能改运行态的端点就多一处鉴权要考虑                                                                                                                                                                               |
| tracing                                                           | 内置                               | 只在 `svc-app::init_tracing`；`EnvFilter` 缺省 `info`；**全仓库只出 compact 单行**，无格式开关 | A7 / E8。见下方「为什么是 compact」                                                                                                                                                                                                   |
| storage：门面 + `init_storage` fail-fast                          | 内置                               | `Arc<dyn Storage>`；trait 只带生命周期四方法                                                   | D1–D3                                                                                                                                                                                                                                 |
| storage：SQLite 双池                                              | 内置                               | 缺省后端                                                                                       | D4                                                                                                                                                                                                                                    |
| storage：PostgreSQL 单池                                          | 内置                               | 同一 `sqlx` feature 集                                                                         | D6「绝不单后端落地」是纪律的核心，模板自己先双后端                                                                                                                                                                                    |
| storage：迁移目录 + `build.rs` + 成对规则                         | 内置                               | 目录先存在，零迁移                                                                             | D5；「加一个仓储」的样例由 README 八步 + bootstrap 三件测试承载，不靠预建表                                                                                                                                                           |
| storage：双后端契约测试 + PG 环境变量门控                         | 内置                               | `<ENV_PREFIX>_TEST_PG_*`；声明了连不上红而不跳                                                 | F3                                                                                                                                                                                                                                    |
| events：`EventBus` / `EventStream`                                | 内置                               | 丢弃式 broadcast，容量 256；`AppEvent` 三个框架级变体                                          | 「通知不载荷」；关停序列需要 `ShuttingDown`                                                                                                                                                                                           |
| metrics：注入式 `Metrics`                                         | 内置                               | 只有 `ticker` 一个分面 + `LastError`                                                           | B10；分面随消费者进场                                                                                                                                                                                                                 |
| metrics：Prometheus / OpenMetrics 导出                            | 不进                               |                                                                                                | 未经论证的依赖；`/v1/service/info` 已能把计数以 JSON 形式给出，导出格式等真实消费者出现再谈                                                                                                                                           |
| shutdown：根令牌 + supervisor 宽限 + 池最后关 + 附加 runtime 逆序 | 内置                               | §4.5                                                                                           | B2–B3                                                                                                                                                                                                                                 |
| http：axum 窄门面 + `/v1/service/{health,ready,info}` + JSON 404  | 内置                               |                                                                                                | E1–E4；健康端点是骨架验收的一部分。就绪判据只看 `storage.health()`                                                                                                                                                                    |
| http：`TraceLayer` + `TimeoutLayer`                               | 内置                               | 两个内联 layer，不建 middleware 目录（E3）                                                     | 请求级日志几乎每个服务第一天就要；`TimeoutLayer` 让悬挂请求不能拖死优雅排空（axum 官方 graceful-shutdown 例子的写法，§8.1）                                                                                                           |
| http：提取器包装（`extract::{Json, Query, Path}`）                | 内置                               | 三个约 15 行的包装 + 单测，零新依赖                                                            | 见下方「为什么破例预铺 extract.rs」                                                                                                                                                                                                   |
| http：CORS                                                        | 不进                               |                                                                                                | 见下方「为什么 CORS 不进」                                                                                                                                                                                                            |
| http：OpenAPI / Swagger 内嵌                                      | 不进                               |                                                                                                | 模板不预设文档工具                                                                                                                                                                                                                    |
| http：Web UI 内嵌（rust-embed）                                   | 不进                               |                                                                                                | 业务                                                                                                                                                                                                                                  |
| tls：`ensure_crypto_provider`（rustls + ring）                    | 内置                               | 约 30 行 + `rustls` 依赖                                                                       | PG `sslmode=require` 经 rustls，缺 provider 是**运行期 panic** 不是编译错误；用户加 `reqwest` 时同样会踩                                                                                                                              |
| HTTP 客户端                                                       | 不进                               |                                                                                                | 无消费者                                                                                                                                                                                                                              |
| testkit                                                           | 内置                               | `TempStorage` / `LogCapture` / `test_port`                                                     | 三个 crate 的测试共用；A3                                                                                                                                                                                                             |
| CLI（`--version` / `--config`）                                   | 内置                               | 手写                                                                                           | 零依赖；两个参数不值一个 clap                                                                                                                                                                                                         |
| 构建串 `BuildInfo`                                                | 内置                               | `option_env!("APP_VERSION")` 等                                                                | H3                                                                                                                                                                                                                                    |
| 工程链：Makefile                                                  | 内置                               | 生成结果三条门禁；模板仓库五条（H5）                                                           | H1                                                                                                                                                                                                                                    |
| 工程链：交叉编译 / 容器 / 打包链                                  | 不进                               |                                                                                                | 部署事实，不是可复用纪律                                                                                                                                                                                                              |
| 工程链：CI 配置                                                   | **生成结果不带**；模板仓库自带一份 | `.github` 在 `cargo-generate.toml` 的 `ignore` 里                                              | 新项目的 CI 平台不定，`make check` 才是门禁的唯一定义。模板仓库自己那份只是在别人的机器上把 `make check` / `make verify` 再跑一遍，门禁内容仍然只写在 Makefile 里                                                                     |
| 工程链：`cargo-generate` 模板化                                   | 内置                               | `cargo-generate.toml` + `hooks/`；`project-name` / `crate_prefix` 两个名字                     | 改名走 cargo-generate 而不是 rename 脚本。代价是模板仓库本身不可直接构建，`make check` 先生成再测；编译错误指向生成目录（相对路径相同，可直接对应回来）                                                                               |
| release profile（`release` + `release-size`）                     | 内置                               |                                                                                                | H2                                                                                                                                                                                                                                    |

### 为什么是 compact 日志

**只出一种形态，不留格式开关。** 三种形态实跑对比过：`json` 一条事件一行但字段全被引号和键名
撑开；`pretty` 一条事件占三行还附 `at file:line`，比 JSON 更难扫；`compact` 一行，结构化字段以
`key=value` 出现，采集器照样切得动。所以「给人读」和「给机器读」在 compact 上并不冲突。

- **testkit 跟着出 compact。** `LogCapture` 装的是自己的 subscriber，本可以出 JSON 让断言按字段找，
  但那等于仓库里存在两种日志形态，且测试断言的形状与生产真出的那行对不上。与生产唯一的差别是关掉
  ANSI——颜色码会把 `key=value` 拆断，留着断言就成了断配色。
- **不做 TTY 自动判别，也不做配置开关。** 两种主流做法（显式 `RUST_LOG_FORMAT` 开关、`format = "auto"`
  TTY 判别）都成立，但都要新增一处配置面，与「不留格式开关」相抵。
- **已知代价**：compact 带 ANSI 颜色，而 `fmt` 不会因为 stdout 被重定向就自动关掉转义序列，
  `cargo run > log.txt` 落盘的文件里会有颜色码。修法是 `.with_ansi(std::io::stdout().is_terminal())`，
  零依赖（`IsTerminal` 自 1.70 进 std），只关颜色不换形态；未做，留待真踩到再说。

**同期评估并拒绝：文件落盘与按 target 拆分日志。**

- 按 target 在写入时拆流，是把查询期的选择提前到写入期——`audit` 单独成文件之后主流里就没有完整
  时间线了，要把一条 audit 事件和触发它的请求对上得按时间戳 join 两个文件。结构化日志的出发点恰恰
  是「一条流、查询时再切」。
- 容器里 runtime 收 stdout、采集器管轮转与分流，systemd 下 journald 管；进程内自己写文件是重复且
  更差的一份（`tracing-appender` 只能按时间轮转，没有按大小、不压缩）。
- 代价落在启动路径上：+1 个 workspace 依赖与 +3 个传递依赖；`init_tracing` 要改成返回 `WorkerGuard`
  由 `main` 持到退出；`main.rs` 里 tracing 与 config 的顺序要翻，翻完配置加载失败就不再进日志文件。
  三处结构性改动换一个模板里没有消费者的功能。

真碰上无采集器的私有化部署，该加的也只是单文件 + 按天轮转，属生成结果里的几十行，不是模板特性。

### 为什么破例预铺 `extract.rs`

它补的不是一个新功能，是模板**已经写下**的一条契约上的洞。`api/src/lib.rs` 说「所有错误 → 同形
信封」，`response.rs` 的 `both_envelopes_share_one_shape` 还在钉着它；今天这句话是真的，因为自带的
三个端点全是 `GET` 且只取 `State`。用户写下第一个 `async fn create(Json(body): Json<CreateReq>)`
的那一刻它变成假的：解析失败发生在 handler **被调用之前**，响应是 `content-type: text/plain` 加
一行纯文本。**这个破坏不需要改模板任何一行代码，也不会让任何一条测试变红**——正是模板该拦的那类
缺陷。成本是约 90 行（含单测）与**零新依赖**。

与 §1.2「不做预铺」的张力是真的。判据是「预铺猜形状、这个不猜」：预铺的害处在于把一个还不知道的
形状写死（预建的表、没人读的 `AppState` 字段、只有一个文件的 `middleware/` 目录）；`extract.rs`
不猜任何形状，它兑现一条已经写下的承诺。不加的替代方案只有「在 README 里写一段警告」，而警告扛不住
一次复制粘贴。

两处实现细节值得记：

- **状态码不拍平。** `JsonRejection` 是个 composite，四个变体状态码各不相同
  （`axum-0.8.9/src/extract/rejection.rs`）：语法坏 400、形状对不上 422、`Content-Type` 不对 415、
  体过大 413。拍平成 400 会让调用方分不出「我发的不是 JSON」「我少了个字段」「我 header 忘了」，
  而这三件事的修法完全不同。改取 `rejection.status()`。附带的好处是模板**不必**在 400 / 415 / 422
  之间选边——那由 axum 按 RFC 定。`HttpError` 的变体因此落成 `Rejected(StatusCode, String)` 而不是
  `BadRequest(String)`：400 只是它的四种取值之一，用变体名钉死其中一种就又回到了拍平。
- **5xx 单独处理。** `PathRejection` 会报 500（`MissingPathParams`、参数个数与路由对不上、类型不
  支持，见 `axum-0.8.9/src/extract/path/mod.rs`），那是本服务的 bug 不是调用方的。这类文案原样回给
  客户端既无用又是信息泄漏，所以在 `HttpError::into_response` 里与 `Internal` 同办：原文进
  `tracing::error!`，对外泛化。

### 为什么 CORS 不进

允许哪些来源是**部署策略**，模板给不出正确缺省。

常见的反驳是「不带凭据时 `allow_origin(Any)` 不放大任何权限，跨源脚本能拿到的东西和一条 curl 一样
多」。这条在公网服务上成立，但在本模板的典型部署形态下不成立：服务跑在内网 / VPN 后 / `localhost`
时，**网络可达性本身就是凭据**。`allow_origin(Any)` 等于允许用户访问的任何网站借用户的浏览器去读
`http://192.168.1.50:8080` 的响应体，浏览器是被利用的中间人（DNS rebinding / 内网 CORS 暴露那一类）。

次要成本：要把 `tower-http` 的 `cors` feature 加回来，而 A2「每个依赖旁要有理由」延伸到 feature，
「模板给不出正确缺省」本身就说明它没有理由。

（附带一条事实，免得有人自己试出来再来问：`allow_credentials(true)` 与 `allow_origin(Any)` 同开会在
建层时 panic，见 `tower-http-0.6.9/src/cors/mod.rs` 的 `ensure_usable_cors_rules`。）

CORS 该长在用户项目里，那时他知道前端部署在哪个域。

---

## 7. 部署布局

```text
<安装根>/              ← 可执行文件所在目录
  config/<服务名>.toml
  data/<服务名>.db
```

三条规则：

1. **安装根 = 可执行文件所在目录，永远。** 不取 cwd：cwd 不是进程自己的属性。systemd 拉起的服务
   cwd 通常是 `/`，按 cwd 推缺省是去建 `/config/`（需要 root，且位置是错的）；从另一个目录启动同一个
   二进制，又会在那里另建一份配置并静默生效，而「换个目录启动就读到另一份配置」查起来没有任何线索。
   也**不跟着 `--config` 走**：单元文件里写 `--config <安装根>/config/x.toml` 只是把缺省值显式写出来，
   不该因此让库换个地方落——「显式写出缺省路径反而改变了行为」是最难查的一类故障。
2. **`--config` / `${env_prefix}_CONFIG` 只改配置文件位置**，其中的相对路径仍相对 cwd（人在 shell 里
   敲的就该是 shell 语义）。`cli::locate_config` 一次返回 `(安装根, 配置路径)`：两者都要问
   `current_exe()`，失败也在同一处报。
3. **配置里的相对路径以安装根为基准**（`path_util.rs`）。基准若取「配置文件所在目录」，`data/` 就会
   钻回 `config/` 里面。「所有路径从同一个锚派生」是 `path_util.rs` 存在的全部理由。

**为什么是两个同级目录，而不是都裸放在可执行文件旁。** 裸放的实际后果是 `target/debug/` 下的
`<服务名>.toml` 与 `<服务名>.db` 混在几十个构建产物中间；而目录本身有用——`config/` 归运维改、
`data/` 归进程写，备份与权限策略也不同，一并放平意味着「拷一份配置」会顺带拷走整个库。

**为什么不按 OS 分出写死的安装前缀**（如 Linux 上固定 `/opt/<name>/`）。它切在错的轴上：真正的
分界是「装出来的包」与「从源码跑起来的」，不是操作系统。按 OS 切的后果是 Linux 开发机上
`cargo run` 去写 `/opt/<name>/config.toml`，首次运行即 `Permission denied`——偏偏 Linux 是最主要的
部署目标。而装包这一侧永远是脚本化的：脚本要把配置写到那个目录，就已经知道路径，顺手传 `--config`
即可，写死的缺省挣不到任何东西。同理也不引入 `install_prefix` placeholder：多问生成者一个问题，
唯一消费者是那个缺省值。

**配置文件名用 `<服务名>.toml` 而非 `app.toml`。** 与 `core::SERVICE_NAME` 同一个串（二进制名、
日志字段、`--version` 已经共用它），配置文件脱离目录之后仍自解释，一台机器上并存多个服务也不重名；
`config/config.toml` 结巴，`app.toml` 又与 crate 里的 `config/app.rs` 撞名。代码里写
`format!("{SERVICE_NAME}.toml")` 而非字面量：项目名进字符串会让该行宽度随名字长短在 rustfmt 阈值
上下翻转，模板便只对部分名字 fmt-clean（H5）。内嵌模板的源文件叫 `default.toml`，把「模板源」与
「运行时那份」在名字上分开。

**签名约定。** `load` / `load_or_init` / `resolve_paths` 都收一个 `root: &Path`，`ConfigStore` 存下
`root` 供热重载沿用同一基准（重载时重新推一次会让「把二进制换个位置」变成路径静默漂移）。基准由
调用方给而不是在管线里推，顺带让 config 模块不碰 `current_exe()`——纯函数，好测。

**已知代价（可接受）。** `cargo run` 的 `config/` 与 `data/` 落在 `target/debug/` 下，`cargo clean`
会一并删掉；想在本地留一份就把 `--config` 指到仓库里（`.gitignore` 的 `*.db` 三条兜的正是这种
情形）。换来的是「同一个二进制在任何目录启动都读同一份配置」，以及启动目录不再长出任何东西。

---

## 8. 借鉴表

完整取证（总表、逐仓库四件事、permalink、入选标准）在 [references.md](references.md)；这里只列结论。

### 8.1 模板与骨架类仓库

| 仓库                                | 入选                                      | 借什么                                                                                                                                                                                                  | 不借什么                                                                                                                                       |
| ----------------------------------- | ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `tokio-rs/axum` `examples/`         | 部分（只借机制）                          | `with_graceful_shutdown` + 外层 `TimeoutLayer`；错误分「客户端可见 / 内部」两类；`fn app() -> Router` 工厂 + `oneshot` 黑盒；sqlx `acquire_timeout` + 启动期 fail-fast                                  | `PgPool` 直连（无门面，违反 D1）；每个 `main` 各自初始化 tracing                                                                               |
| `loco-rs/loco`                      | 部分（关停与配置形态）                    | server 先退再关队列的顺序；`await_shutdown_then_run_hook` 接收任意 future 便于测试；启动期权限探测                                                                                                      | sea-orm 绑定；15 方法的 `Hooks` 大 trait（违反 A5）；scheduler 裸 spawn 不受监管（违反 B1）；`{env}.local.yaml` 叠加（与 C5 的单一真值源相抵） |
| `LukeMathWalker/zero-to-production` | 否（维护超 12 个月；actix-web；单 crate） | 只记反例：`select!` 先退者胜、另一 task 不取消不排空——正是 B1 / B2 要补的洞                                                                                                                             | 全部                                                                                                                                           |
| `launchbadge/realworld-axum-sqlx`   | 否（AGPL；2022 后无维护；无关停）         | 不借任何代码                                                                                                                                                                                            | 全部                                                                                                                                           |
| `rust-lang/crates.io`               | 是                                        | 二进制入口显式 `runtime::Builder` 设 `worker_threads` / `max_blocking_threads`（后者来自配置）——**直接印证 §4.1 / §4.6**；tracing 只在入口初始化；worker `RunHandle` join 并记录 panic；叶子 crate 命名 | diesel；`std::thread::spawn` 的 metrics 线程不受监管                                                                                           |
| `juspay/hyperswitch`                | 是（只借存储门面）                        | `dyn StorageInterface` + `MockDb` 双实现验证 D1 路线                                                                                                                                                    | actix-web；supertrait 拆得过细；`Box<dyn>` 每次 clone。测试替身走「失败装饰器」形态，不做整套 mock                                             |

### 8.2 多 runtime 案例

| 项目                                                    | 为什么分                     | 绑定方式                                                                                                      | 关停                                                                                        | 落到本设计的哪里                                                                                          |
| ------------------------------------------------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `influxdata/influxdb`（influxdb3，MIT OR Apache-2.0）   | CPU 计算不能阻塞 IO 轮询     | `DedicatedExecutor` **值类型**：`JoinSet::spawn_on(task, &handle)`；CPU 侧经 `spawn_io` 把 IO 送回 IO runtime | 专用 runtime 在独立 OS 线程上 `shutdown_timeout(5min)`；两段式令牌                          | 印证 §4.3「值类型而非全局静态」与 §4.5「同步关、独立宽限」；`spawn_io` 形态不采纳（本模板的隔离粒度是面） |
| `quickwit-oss/quickwit`（Apache-2.0）                   | CPU 密集 actor 与 IO 分离    | `Actor::runtime_handle()` 默认 `Handle::current()`，CPU 型 actor 覆写；`static OnceLock<HashMap<…>>`          | 从不显式关，`process::exit`                                                                 | 「默认当前、只有需要的才覆写」与 §4.3 `[面].runtime` 缺省主 runtime 同一口径；`static` 与不关停两点不借   |
| `risingwavelabs/risingwave`（Apache-2.0）               | 按子系统隔离，各自线程数可配 | 每个 manager 持 `Arc<BackgroundShutdownRuntime>`，线程名带子系统名                                            | 注释明言「父 runtime 里不能直接 drop 嵌套 runtime」→ `ManuallyDrop` + `shutdown_background` | 线程命名 `"<name>-<seq>"`（§4.6）；`shutdown_background` 是 §4.5 不选的那条路（看不出顺序）               |
| `databendlabs/databend`（许可混合，只作参考，不借代码） | IO 与计算分池                | 三个全局 runtime 放 `GlobalInstance`                                                                          | `Dropper` 独立线程 join；「同 runtime 的 worker 上 join 会死锁」                            | 同一硬约束的第三个佐证（§4.5）                                                                            |

结论：**四个生产项目分 runtime 的理由全部落在 §4.2 的第 1、3 条**（CPU 密集不阻塞 IO、隔离延迟敏感
面），没有一个是为「逻辑分开」而分。绑定方式上，值类型 + 「默认当前 runtime、只有需要的才覆写」是
共识；关停上，「嵌套 runtime 不能在 async 上下文里 drop，要么后台关、要么独立线程同步关」是三家都
踩过的同一条约束。
