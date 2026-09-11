# GitHub 调研：Rust Web 模板的参考仓库

> 目的：验证与补强 [architecture.md](architecture.md) 的设计，不是收集清单。
> 入选标准（同时满足）：近 12 个月有维护；workspace 多 crate 分层（或单 crate 但有明确分层）；有明确的任务监管或优雅关停实现；有存储抽象或模板意图；许可证 MIT / Apache-2.0。不满足的如实标出，不降标准凑数。
> 方法：元数据与 HEAD SHA 取自 GitHub REST API；文件内容按同一 commit SHA 拉取，行号实际核对。所有 permalink 带 SHA。
>
> **这是一份 2026-09-10 的快照。** 所有行号与结论都钉在文中记录的 commit SHA 上；上游仓库此后的改动不会反映在这里。要复核某一条，顺着 permalink 走。

## A. 模板与骨架类仓库

### A.1 总表

| 仓库                                | 最近 commit | 许可证            | 多 crate 分层                                                            | 关停 / 监管                                                       | 存储抽象 / 模板意图                                            | 入选                                                     |
| ----------------------------------- | ----------- | ----------------- | ------------------------------------------------------------------------ | ----------------------------------------------------------------- | -------------------------------------------------------------- | -------------------------------------------------------- |
| `tokio-rs/axum` `examples/`         | 2026-09-09  | MIT               | 框架是 workspace；每个 example 独立 crate                                | 有 `with_graceful_shutdown` 示例                                  | **不满足**：`PgPool` 直连，无门面，无模板意图                  | 部分：只借机制                                           |
| `loco-rs/loco`                      | 2026-09-08  | Apache-2.0        | **不满足**：框架单 crate + `xtask` / `loco-gen`；生成的 app 也是单 crate | 有：server 优雅关停 + worker 二次 ctrl-c 强退；scheduler 裸 spawn | 有：sea-orm 绑定、队列三后端、`loco new`                       | 部分：只借关停与配置形态                                 |
| `LukeMathWalker/zero-to-production` | 2024-09-01  | Apache-2.0        | **不满足**：单 crate                                                     | **不满足**：`select!` 先退者胜，无取消、无排空                    | **不满足**：`PgPool` 直连                                      | **否**（维护超 12 个月；actix-web）——只作反例记录        |
| `launchbadge/realworld-axum-sqlx`   | 2022-03-12  | **AGPL-3.0**      | 不满足                                                                   | **不满足**：`Server::bind().serve()` 无关停                       | 否                                                             | **否**（许可证 / 维护 / 关停三条全不满足）——不借任何代码 |
| `rust-lang/crates.io`（补）         | 2026-09-10  | MIT OR Apache-2.0 | 是：根 app + `crates/*` 29 个叶子 crate                                  | 有：server 优雅关停；worker `RunHandle` join                      | 部分：独立 database crate（diesel），非 trait 门面；无模板意图 | 是：生产级 workspace 参照                                |
| `juspay/hyperswitch`（补）          | 2026-09-09  | Apache-2.0        | 是：`crates/*`，含 `router` / `storage_impl`                             | 有：actix `shutdown_timeout` + oneshot 故障停服                   | 是：`dyn StorageInterface` + `MockDb`                          | 是，但只借存储门面（actix-web、体量大）                  |

### A.2 `tokio-rs/axum` examples（SHA `af1345b5`）

- **解决了什么**：每个 axum 机制的最小正确用法；不是模板。
- **借什么**：
  - `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal())`，外层挂 `TimeoutLayer` 防悬挂请求拖死排空；`shutdown_signal` 是 ctrl_c / SIGTERM 的 `select!`。
  - 错误分两类：用户输入错误直接回状态码；内部错误只回泛化文案，把 `Arc<AppError>` 塞进 response extensions，由中间件统一 `tracing::error!`。
  - `#[derive(FromRequest)] #[from_request(via(axum::Json), rejection(AppError))]` 统一 JSON 拒绝格式。
  - 测试：`fn app() -> Router` 工厂 + `ServiceExt::oneshot` 黑盒；端口 0 真 TCP；`MockConnectInfo`。
  - sqlx：`acquire_timeout` + 启动期 `expect` fail-fast；自定义 extractor 经 `FromRef` 从 State 取连接。
- **不借什么、为什么**：例子直接持有 `PgPool`（无门面，违反 D1）；tracing 在每个 `main` 初始化（模板只在装配层一处）；无任务面。
- **证据**：
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/graceful-shutdown/src/main.rs#L35-L51
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/graceful-shutdown/src/main.rs#L54-L76
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/axum/src/serve/mod.rs#L240
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/anyhow-error-response/src/main.rs#L35-L61
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/error-handling/src/main.rs#L140-L151
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/error-handling/src/main.rs#L163-L197
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/error-handling/src/main.rs#L211-L219
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/testing/src/main.rs#L35-L52
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/testing/src/main.rs#L67-L82
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/testing/src/main.rs#L122-L147
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/sqlx-postgres/src/main.rs#L42-L47
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/examples/sqlx-postgres/src/main.rs#L75-L91
  - https://github.com/tokio-rs/axum/blob/af1345b53a259b0990be1ff853f9b56c05040ef7/Cargo.toml#L1-L3

### A.3 `loco-rs/loco`（SHA `23639d1e`；HEAD 下已无 `starters/`，模板意图由 `loco-new` / `loco-gen` 承担）

- **解决了什么**：Rails 式「一人框架」：`loco new` 生成 app，`Hooks` trait 挂接路由 / worker / 任务 / 迁移。
- **借什么**：
  - 关停顺序：server 先退，再 `queue.shutdown()`；`select!{handle, shutdown_signal()}` 实现「再按一次 ctrl-c 强退」；`await_shutdown_then_run_hook` 接收任意 future，便于用已完成 future 测试关停；`Hooks::on_shutdown` 应用级钩子。
  - 配置：`{env}.yaml` + 可选 git-ignore 的 `{env}.local.yaml` 深合并；YAML 内 `<%= %>` 模板取环境变量。
  - DB：`verify_access` 启动期权限探测；`converge` 按 `auto_migrate` / `dangerously_recreate` / `dangerously_truncate` 开关执行。
  - 队列三后端（Postgres / SQLite / Redis）藏在对象安全 trait 后。
- **不借什么、为什么**：sea-orm 绑定（模板是 sqlx 门面）；15 个方法的 `Hooks` 大 trait（违反 A5「不抽象协议」）；scheduler 裸 `tokio::spawn` 只记日志不受监管（违反 B1）；脚手架 / 生成器（模板不是框架）。
- **证据**：
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/Cargo.toml#L1-L3
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/Cargo.toml#L56-L61
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/boot.rs#L91-L149
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/boot.rs#L151-L165
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/boot.rs#L183-L197
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/boot.rs#L578-L600
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/app.rs#L422-L436
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/app.rs#L601
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/config/mod.rs#L144-L190
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/config/mod.rs#L192-L200
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/environment.rs#L30-L31
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/db/connect.rs#L61-L80
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/db/migrate.rs#L17-L37
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/bgworker/mod.rs#L147-L148
  - https://github.com/loco-rs/loco/blob/23639d1e360dbc618073642b507d6f8664adbaff/src/bgworker/mod.rs#L509

### A.4 `LukeMathWalker/zero-to-production`（SHA `970987c5`）——不入选，只记反例与两个可借形态

- **解决了什么**：教科书式 actix-web 服务：分层配置、bunyan tracing、每测试独立 DB。
- **可借的形态**（非代码）：`Application::build / port() / run_until_stopped` 三段，bind 端口 0 让测试拿真实端口；telemetry 把 `get_subscriber`（组装）与 `init_subscriber`（`set_global_default`）分离；`report_exit` 对 `Result<Result<_>, JoinError>` 三分支日志。
- **反例**：`main` 只 `select!` 两个 task，先退者胜，另一个 task 既不取消也不排空——正是 `JoinSet` + `CancellationToken` 级联 + 限时收割要补的洞。
- **不入选原因**：最近 commit 2024-09-01（超 12 个月）；actix-web；单 crate；`PgPool` 直连。
- **证据**：
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/Cargo.toml#L1-L17
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/src/main.rs#L13-L21
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/src/main.rs#L26-L48
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/src/startup.rs#L23-L63
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/src/configuration.rs#L83-L110
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/src/telemetry.rs#L15-L38
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/tests/api/helpers.rs#L15-L25
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/tests/api/helpers.rs#L182-L208
  - https://github.com/LukeMathWalker/zero-to-production/blob/970987c5f793af6fc8e557731c9bbb23b620451e/src/issue_delivery_worker.rs#L8-L25

### A.5 `launchbadge/realworld-axum-sqlx`（SHA `f1b25654`）——不入选

- **解决了什么**：RealWorld 规范的 axum 0.3 + sqlx 0.5 参考实现（2022）。
- **不入选原因**：AGPL-3.0（模板要能被任意项目复用，许可证是硬标准）；2022-03-12 后无维护；无关停；axum 0.3 API 已过时。**不借任何代码。** 它的「DB 唯一约束错误 → 422」思路是通用知识，模板的等价物是 `StorageError::UniqueViolation → 409`。
- **证据**：
  - https://github.com/launchbadge/realworld-axum-sqlx/blob/f1b25654773228297e35c292f357d33b7121a101/LICENSE#L1-L2
  - https://github.com/launchbadge/realworld-axum-sqlx/blob/f1b25654773228297e35c292f357d33b7121a101/Cargo.toml#L1-L17
  - https://github.com/launchbadge/realworld-axum-sqlx/blob/f1b25654773228297e35c292f357d33b7121a101/src/main.rs#L31-L48
  - https://github.com/launchbadge/realworld-axum-sqlx/blob/f1b25654773228297e35c292f357d33b7121a101/src/http/error.rs#L18-L77

### A.6 `rust-lang/crates.io`（补，SHA `990b7528`）

- **解决了什么**：crates.io 生产后端：根 app 双二进制（`server` / `background_worker`）+ 29 个叶子 crate。
- **借什么**：
  - 显式 `runtime::Builder::new_multi_thread()` 设 `worker_threads` / `max_blocking_threads`（后者来自配置），`rt.block_on(axum::serve(...).with_graceful_shutdown(shutdown_signal()))`——**直接印证设计 §4.1 / §4.6「主 runtime 手动构建、线程预算配置化」**。
  - tracing / sentry 只在二进制入口初始化。
  - worker：`Runner::start` 返回 `RunHandle { Vec<JoinHandle> }`，`wait_for_shutdown` 做 `join_all` 并记录 panic；`shutdown_when_queue_empty` 供测试。
  - 叶子 crate 命名（`crates_io_database` / `test_db` / `test_utils` / `env_vars`）与模板的 core / storage / testkit 对应。
- **不借什么、为什么**：diesel-async；metrics 用 `std::thread::spawn` 死循环不受监管；listener panic 只记日志不重启；无模板意图。
- **证据**：
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/Cargo.toml#L1-L12
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/src/bin/crates-io/server.rs#L18-L26
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/src/bin/crates-io/server.rs#L60-L90
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/src/bin/crates-io/server.rs#L92-L111
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/src/bin/crates-io/server.rs#L113-L128
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/crates/crates_io_worker/src/runner.rs#L64-L66
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/crates/crates_io_worker/src/runner.rs#L72-L121
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/crates/crates_io_worker/src/runner.rs#L139-L151
  - https://github.com/rust-lang/crates.io/blob/990b75280f1876796211688c9ed52bb7796e005a/src/bin/crates-io/background_worker.rs#L135-L151

### A.7 `juspay/hyperswitch`（补，SHA `2dd05cbf`）

- **解决了什么**：支付路由服务；`crates/router` 只经 `dyn StorageInterface` 访问 `crates/storage_impl`。
- **借什么**：`StorageInterface: Send + Sync + DynClone + 各聚合子 trait`，`impl StorageInterface for Store` 与 `MockDb` 两实现——印证 D1「`Arc<dyn Storage>` 门面 + 测试替身」路线；`oneshot` 通道让依赖（redis）故障时主动 `stop_server`。
- **不借什么、为什么**：actix-web；supertrait 按聚合拆得过细（模板保持单一 `Storage` 门面 + 按切片加访问器）；`Box<dyn>` 每次 clone；体量。
- **证据**：
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/Cargo.toml#L1-L3
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/LICENSE#L2-L3
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/crates/storage_impl/Cargo.toml#L1-L3
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/crates/router/src/db.rs#L97-L102
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/crates/router/src/db.rs#L239-L251
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/crates/router/src/lib.rs#L397-L414
  - https://github.com/juspay/hyperswitch/blob/2dd05cbfbe9cdba4ccd0dfdc6347c6be8ee2c3af/crates/router/src/lib.rs#L475-L507

### A.8 启示与采纳状态

| #   | 启示                                                                                                        | 来源                                  | 采纳状态                                                                                                                                                         |
| --- | ----------------------------------------------------------------------------------------------------------- | ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `with_graceful_shutdown` 要配外层 `TimeoutLayer`，否则悬挂连接拖死排空                                      | axum graceful-shutdown                | **采纳**：模板 HTTP 面两个内联 layer——`TraceLayer` + `TimeoutLayer`（tower-http `timeout` feature，无新 crate）；`wait_for_shutdown` 的超时 abort 仍是最后一道网 |
| 2   | 错误分「客户端可见 / 内部」两类；内部错误经 response extensions 交中间件统一记日志，`IntoResponse` 无副作用 | axum error-handling                   | **部分采纳**：两类之分与模板的 `HttpError` 一致；不加日志中间件，`HttpError::Internal` 在 `error.rs` 一处记 `tracing::error!`（E5 集中映射）                     |
| 3   | `fn app(state) -> Router` 工厂 + `oneshot` 黑盒为主，端口 0 真 TCP 为辅                                     | axum testing / z2p                    | **已等价**：纪律 F1 / F2                                                                                                                                         |
| 4   | `select!` 「先退者胜」是反例；`report_exit` 三分支日志可借                                                  | z2p                                   | **已等价**：`TaskSupervisor::log_task_exit` 已是三分支（`core/src/task/supervisor.rs`）                                                                          |
| 5   | 关停顺序：HTTP 停止接入 → worker 不取新活 → 限时等待 → 超时强退；loco 用二次 ctrl-c 强退                    | loco / crates.io                      | **已等价**：根令牌一次 cancel 时 `axum::serve` 立即停止接入、在途请求在宽限内完成；强退用超时 abort 而不是二次信号                                               |
| 6   | 配置 `base` + `{env}` + 环境变量前缀覆盖；loco 的 git-ignore `{env}.local.yaml` 叠加                        | z2p / loco                            | **不采纳**：单配置文件 + `${env.NAME:default}` 占位符已覆盖环境差异；叠加文件会给热重载引入第二个真值源，与 C5「watch 里的配置永远等于生效中的配置」相抵         |
| 7   | 启动 fail-fast 三件套：`acquire_timeout` / 权限探测 / 嵌入迁移 `run().await?`                               | axum sqlx-postgres / loco / realworld | **已等价**：D2 / D4（`acquire_timeout` 5s、迁移、`health()` 自检覆盖权限探测）                                                                                   |
| 8   | trait 对象存储门面 + `MockDb` 双实现是生产验证过的路径，但 supertrait 拆分要克制                            | hyperswitch                           | **采纳其判断**：单一 `Storage` 门面；测试替身走「失败装饰器」形态（见 `api/Cargo.toml` 的 dev-deps 注释）而非整套 `MockDb`                                       |
| 9   | 显式 `runtime::Builder` 设线程数、tracing 只在二进制入口初始化                                              | crates.io                             | **采纳**：直接印证设计 §4.1 / §4.6——主 runtime 手动 `Builder` 构建而不是 `#[tokio::main]`，线程预算来自配置                                                      |

## B. 生产里真正跑多个 Tokio runtime 的项目

> 入选标准：生产级、近 12 个月有维护、MIT / Apache-2.0、代码里**有意**构造 ≥ 2 个 runtime 且理由有据可查、有可辨认的关停处理。
> 取证通道：与 A 部分相同（jsDelivr 按 SHA 拉取 + `cat -n` 核对）。`apache/datafusion`、`vectordotdev/vector` 超出 CDN 体积限制未能取到原文，只作文档参照或按摘要标注。

### B.1 总表

| 项目                               | 最近 commit | 许可证                                                                                      | runtime 数量与名字                                                                      | 分的理由                   | 关停处理                                                       | 入选                               |
| ---------------------------------- | ----------- | ------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | -------------------------- | -------------------------------------------------------------- | ---------------------------------- |
| `influxdata/influxdb`（influxdb3） | 2026-08-17  | MIT OR Apache-2.0                                                                           | 3：主 IO runtime + `datafusion` + `datafusion_write_path`（各一个 `DedicatedExecutor`） | CPU 计算不阻塞 IO 轮询     | `Notify` → `shutdown_timeout(5min)` → oneshot；`Drop` 兜底告警 | **是**                             |
| `quickwit-oss/quickwit`            | 2026-09-08  | Apache-2.0                                                                                  | 3：`main_runtime_thread` + `blocking-*` + `non-blocking-*`                              | CPU 密集 actor 与 IO 分离  | 无显式关停，`process::exit`                                    | **是**                             |
| `risingwavelabs/risingwave`        | 2026-09-10  | Apache-2.0                                                                                  | ≥ 5：`rw-main` / `rw-streaming` / `rw-batch` / `rw-compaction` / `rw-batch-local`       | 子系统隔离，各自线程数可配 | `BackgroundShutdownRuntime` 的 `Drop` → `shutdown_background`  | **是**                             |
| `databendlabs/databend`            | 2026-09-10  | 核心 Apache-2.0；`src/query/ee` 等目录 Elastic-2.0（混合，LICENSE 文件 permalink 未能核实） | 3：`IO-worker` / `control-worker` / `query-worker`                                      | IO 与计算分池              | `Dropper` 独立线程 join；debug 构建 3s 超时                    | 参考（许可混合，单列；不借代码）   |
| `tikv/tikv`                        | 2026-09-08  | Apache-2.0                                                                                  | 只核实到一个 1 线程 debug 池，其余散落各组件                                            | 未集中说明                 | 未核实                                                         | 否（理由不可查，超出文件预算）     |
| `apache/datafusion`                | 2026-09-10  | Apache-2.0                                                                                  | 库，非服务                                                                              | —                          | —                                                              | 否（仅作文档参照）                 |
| `vectordotdev/vector`              | 2026-09-09  | MPL-2.0                                                                                     | 单 runtime（`vector-worker`，据搜索摘要，原文未取到）                                   | —                          | —                                                              | 否（许可证不符，且不是多 runtime） |

### B.2 `influxdata/influxdb`（influxdb3）——`DedicatedExecutor`（SHA `693b1fd1`）

1. **为什么分**（原话）：*"For CPU bound work, such as DataFusion plan execution, it is important to run on a separate thread pool to avoid blocking the I/O handling for extended periods of time in order to avoid long poll latencies"*；并解释 tokio 协作调度下 CPU 任务会饿死 IO。
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L49-L83
2. **绑定方式**：专用 executor **值类型**。`DedicatedExecutor::spawn` 用 `JoinSet::spawn_on(task, &handle)` 投到专用 runtime，`JoinSet` 提供 drop-cancel；执行线程 `on_thread_start` 把**创建时的当前 runtime** 登记为 thread-local `IO_RUNTIME`，CPU 侧经 `spawn_io(fut)` 显式把 IO 送回 IO runtime。`serve.rs` 构造两个实例（查询 / 写路径各一）。
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L299-L337
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L199-L211
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/io.rs#L9-L71
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/influxdb3/src/commands/serve.rs#L916-L970
3. **跨 runtime 通信与坑**：`CrossRtStream` 用 `mpsc::channel(1)` 把 CPU runtime 上算出的流拉到 IO runtime（首行注释：*"This is critical so that CPU heavy loads are not run on the same runtime as IO handling"*）。坑：未登记 IO runtime 时 `get_io_runtime` panic（说明把 CPU 活跑到了 IO 池）；IO runtime 已关时 `spawn_io` panic "IO runtime was shut down"；文档明示 *"Cannot drop a runtime in a context where blocking is not allowed"*。
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/iox_query/src/exec/cross_rt_stream.rs#L1-L36
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L92-L98
4. **关停顺序与超时**：专用 runtime 跑在**独立 OS 线程**内，`block_on(notified())` 等 `Notify`，醒后 `runtime.shutdown_timeout(SHUTDOWN_TIMEOUT = 5min)`，再经 oneshot 通知完成；`shutdown()` 置 `handle = None` 拒绝新任务，`join()` 等 oneshot；`State::drop` 若未调用 shutdown 则告警并自动触发。进程级：`ShutdownManager` 先取消 backend token、等所有注册组件 `complete()`，再取消 frontend token（先 WAL 落盘后停 HTTP / gRPC）。
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L224-L245
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L137-L152
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/core/executor/src/lib.rs#L340-L380
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/influxdb3_shutdown/src/lib.rs#L1-L16
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/influxdb3_shutdown/src/lib.rs#L121-L158
   - https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/influxdb3/src/commands/serve.rs#L1399-L1479
5. **许可证与背景**：https://github.com/influxdata/influxdb/blob/693b1fd1b96cdcb980cf76a1004c0b3f1b46db48/Cargo.toml#L134 ；博文 https://www.influxdata.com/blog/using-rustlangs-async-tokio-runtime-for-cpu-bound-tasks/

### B.3 `quickwit-oss/quickwit`——`RuntimeType` 枚举（SHA `a39730c5`）

1. **为什么分**（原话）：Blocking = *"only used as a nice thread pool with the interface as tokio tasks… should not be used to run tokio io operations"*；NonBlocking = *"Task are expect to yield within 500 micros"*；线程分配：*"Non blocking task are supposed to be io intensive… the blocking actors are cpu intensive. We allocate almost all of the threads to them."*
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-common/src/runtimes.rs#L52-L70
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-common/src/runtimes.rs#L89-L111
   - https://quickwit.io/blog/quickwit-actor-framework
2. **绑定方式**：枚举选择 + `Handle`。全局 `static RUNTIMES: OnceLock<HashMap<RuntimeType, Runtime>>`；`Actor` trait 的 `fn runtime_handle(&self) -> Handle` 默认 `Handle::current()`，CPU 型 actor（Indexer、DocProcessor）覆写为 `RuntimeType::Blocking.get_runtime_handle()`；`SpawnBuilder::spawn` 读该 handle 并 `spawn_named_task_on`。只有启用 Indexer / Janitor / ControlPlane / Compactor 服务时才初始化这两个 runtime。
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-common/src/runtimes.rs#L26
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-actors/src/actor.rs#L119-L121
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-actors/src/spawn_builder.rs#L167-L185
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-indexing/src/actors/indexer.rs#L395-L397
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-cli/src/lib.rs#L217-L230
3. **跨 runtime 通信与坑**：actor 邮箱底层是 `flume`（与 runtime 无关的 MPMC），天然跨 runtime。坑：未初始化就取 handle 会 panic；Blocking runtime 默认 `disable_lifo_slot()`（`QW_DISABLE_TOKIO_LIFO_SLOT`），规避 LIFO slot 导致的饥饿。
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-actors/src/channel_with_priority.rs#L18
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-common/src/runtimes.rs#L121-L128
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-common/src/runtimes.rs#L166-L185
4. **关停**：两个 actor runtime 是 `static`，**从不显式 shutdown**；主 runtime 由 `main()` 手工 `Builder` 构建后 `rt.block_on(main_impl())`，末尾 `telemetry_handle.shutdown().await; std::process::exit(code)`。优雅性靠 actor 层的 KillSwitch / quit，不靠 runtime 层。
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-cli/src/main.rs#L53-L63
   - https://github.com/quickwit-oss/quickwit/blob/a39730c5cdcd1a4fe798403737ae293999ea21f8/quickwit/quickwit-cli/src/main.rs#L150-L151

### B.4 `risingwavelabs/risingwave`——每子系统一个嵌套 runtime（SHA `4e0e84ae`）

1. **为什么分**（原话）：`CompactionExecutor` *"is a dedicated runtime for compaction's CPU intensive jobs"*；frontend `compute_runtime` *"Runtime for compute intensive tasks in frontend"*；`actor_manager.runtime` *"Runtime for the streaming actors"*；主 runtime 文档注明全局线程数 *"can still be overridden by per-module runtime worker thread settings"*。
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/storage/src/hummock/compactor/compaction_executor.rs#L22-L45
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/frontend/src/session.rs#L189-L191
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/utils/runtime/src/lib.rs#L60-L64
2. **绑定方式**：每个 manager 持有 `Arc<BackgroundShutdownRuntime>`（`Deref<Target = Runtime>`），调用点 `self.runtime.spawn(...)`；构造处 `Builder::new_multi_thread().thread_name("rw-streaming" / "rw-batch" / "rw-compaction")`，线程数来自配置项。
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/stream/src/task/barrier_worker/mod.rs#L1104-L1114
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/batch/src/task/task_manager.rs#L73-L83
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/batch/src/task/task_manager.rs#L137-L140
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/storage/src/hummock/compactor/compaction_executor.rs#L49-L56
3. **跨 runtime 通信与坑**：返回 `JoinHandle` 让主 runtime `await`；batch 结果经 `tokio::sync::mpsc` 交给 gRPC exchange。核心坑写在包装类型注释：*"directly dropping a nested runtime is not allowed in a parent runtime"*，因此用 `ManuallyDrop` + `shutdown_background()`。
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/common/src/util/runtime.rs#L20-L35
4. **关停**：`main_okk` 建 `rw-main` runtime，`CancellationToken` 顶层级联；SIGINT 一次取消、二次强杀；SIGTERM 只取消（交给 k8s SIGKILL 兜底）；future 返回后 `runtime.shutdown_background()` 再退出。compute node 收到取消后：向 meta 注销 → `stream_mgr.shutdown().await` → 注释明言**不 join tonic**（长连接永不关）。子 runtime 无独立超时，靠 `Drop` 后台关。
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/utils/runtime/src/lib.rs#L128-L183
   - https://github.com/risingwavelabs/risingwave/blob/4e0e84ae5b2c2386cfb89d39ab1312620719f96f/src/compute/src/server.rs#L529-L545

### B.5 `databendlabs/databend`（参考，许可混合，不借代码；SHA `be22acf4`）

三个全局 runtime 由 `GlobalServices::init` 顺序创建：`GlobalIORuntime` → `GlobalControlRuntime`（2 线程）→ `GlobalQueryRuntime`；存放于 `GlobalInstance`（`OnceCell<TypeMap>`），生产环境不释放。`Runtime` 包装类型内含 `Dropper`：drop 时向「wait-to-drop」线程发停信号并 join；若 drop 发生在**本 runtime 的 worker 上则不 join**（注释：*"Joining it from one of the same runtime's workers would deadlock"*）；debug 构建 `shutdown_timeout(3s)`。跨 runtime 用 `block_in_place(|| handle.block_on(fut))`。服务关停：先优雅停各 service → 向 meta 注销 → 会话 `graceful_shutdown(timeout)` → 强制停 service；`Drop` 兜底 5s。

- https://github.com/databendlabs/databend/blob/be22acf45556a3e078d70239fd5a5aa21e76c7c3/src/common/base/src/runtime/global_runtime.rs#L35-L82
- https://github.com/databendlabs/databend/blob/be22acf45556a3e078d70239fd5a5aa21e76c7c3/src/query/service/src/global_services.rs#L113-L116
- https://github.com/databendlabs/databend/blob/be22acf45556a3e078d70239fd5a5aa21e76c7c3/src/common/base/src/runtime/runtime.rs#L305-L345
- https://github.com/databendlabs/databend/blob/be22acf45556a3e078d70239fd5a5aa21e76c7c3/src/common/base/src/runtime/runtime.rs#L449-L455
- https://github.com/databendlabs/databend/blob/be22acf45556a3e078d70239fd5a5aa21e76c7c3/src/query/service/src/servers/server.rs#L73-L81
- https://github.com/databendlabs/databend/blob/be22acf45556a3e078d70239fd5a5aa21e76c7c3/src/binaries/query/entry.rs#L405-L410
- 博客（页面被拦，引文来自搜索摘要）：https://www.databend.com/blog/engineering/rust-for-big-data-how-we-built-a-cloud-native-mpp-query-executor-on-s3-from-scratch/

### B.6 DataFusion 官方说明（仅文档参照，原文件未取到）

docs.rs 首页「Thread Scheduling, CPU / IO Thread Pools, and Tokio Runtimes」一节：*"if you need low p99 latencies responding to network requests, it is likely you need to use a different Runtime for DataFusion plans. The thread_pools example has an example of how to do so."* 示例 `datafusion-examples/examples/thread_pools.rs`；配套 issue #12393（文档化）、#13692（讨论用 `block_in_place` 替代分 runtime 的取舍）。
- https://docs.rs/datafusion/latest/datafusion/ ； https://github.com/apache/datafusion/issues/12393 ； https://github.com/apache/datafusion/issues/13692

### B.7 启示与采纳状态

| #   | 启示                                                                                                      | 来源                              | 采纳状态                                                                                                                                                                                                                 |
| --- | --------------------------------------------------------------------------------------------------------- | --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1   | 能力位应是「专用 executor 值类型」而不是全局静态：可 clone、可 join、`Drop` 兜底告警                      | influxdb3 vs quickwit             | **采纳**：设计 §4.3 的 `Runtimes` / `Executors` 是值类型，由 `main` 持有、`RuntimeState` 持 `Executors`；不用 `static OnceLock`                                                                                          |
| 2   | 绑定方式用「任务声明自己要哪个 runtime」，默认 `Handle::current()`，只有 CPU 型任务覆写                   | quickwit                          | **等价形态**：模板把声明放在面的配置段 `[面].runtime`（缺省主 runtime），由装配层解析；代码里覆写与配置里覆写是同一件事的两种落位，模板选配置（§4.3 理由、§9.12）                                                        |
| 3   | CPU runtime 上禁止直接 IO，提供 `spawn_io` 回送，用 panic 式护栏而不是约定                                | influxdb3                         | **不采纳（本期）**：模板的隔离粒度是**面**而不是任务，每个附加 runtime 都 `enable_all`，面内 IO 就地跑；「CPU / IO 拆到任务粒度」的服务再引入 `DedicatedExecutor` 形态，设计 §4.2 的准入清单已把这种场景归到「先量再开」 |
| 4   | 跨 runtime 传流用有界 channel + 驱动 future；`block_in_place(handle.block_on)` 仅限 worker 线程，不作默认 | influxdb3 / databend              | **采纳其约束**：§4.4 规则 1 与 3 已是这个口径；补一句「`block_in_place` 只在多线程 runtime 的 worker 线程上合法，模板不用」                                                                                              |
| 5   | 嵌套 runtime 只能 `shutdown_background` 或在独立线程上关；「同 runtime worker 上 join 会死锁」            | risingwave / databend / influxdb3 | **采纳**：正是 §4.5「附加 runtime 在 `block_on` 返回后同步关」的理由；influxdb3 的「专用 runtime 放独立 OS 线程 + `shutdown_timeout`」是同一解法的另一落位，写进 `rt.rs` 模块注释                                        |
| 6   | 关停顺序 = backend 组件先、frontend 后、专用 runtime 最后；专用 runtime 的超时独立可配                    | influxdb3 / databend              | **部分采纳**：「专用 runtime 最后、独立宽限」已在 §4.5 / §9.11；「backend 先于 frontend 的两段式令牌」不采纳（本期）——模板没有必须先落盘的后端，单一根令牌 + 池最后关已够，列入 §9.14                                    |
