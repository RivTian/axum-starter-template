# {{project-name}}

由 axum-starter-template 生成的 Rust Web 服务骨架：多 crate 单向分层、一个多线程 Tokio
runtime、受监督的顶层任务面、根令牌级联关停、可热重载的配置。这份 README 是这个服务
的说明书，改成你的服务时把它一起改。

## 第一天

1. `make check` 三条门禁全绿；`cargo run -p {{crate_prefix}}-app`，看第一条日志的构建串，
   `curl localhost:8080/v1/service/ready` 得 200（配置与 SQLite 库落在二进制旁的两个同级
   目录里，即 `target/debug/config/{{crate_name}}.toml` 与
   `target/debug/data/{{crate_name}}.db`），
   Ctrl-C 看关停日志里每个任务的 `stopped` 与最后的 `storage closed`。
2. 改 `Cargo.toml` 的 `[workspace.package]`：`version` / `authors`。
3. 把示例任务面 `ticker`（`worker/`）改成你的第一个面，或者删掉它（见「怎么加一个任务面」）。
4. 需要库表时按「怎么加一个仓储」走垂直切片。
5. 不碰 `[runtime.extra]`，除非已经量出某个面在抢主 runtime 的线程。

## 布局

```text
core/      叶子内核：配置、错误基座、事件总线、指标、顶层任务监管、构建元数据
storage/   持久化层：Storage 门面 + SQLite / PostgreSQL 双后端 + 迁移
worker/    示例任务面 ticker
api/       HTTP 展示层：axum 窄门面、/v1/service/* 探针与自省、响应契约、JSON 404
app/       装配层：CLI、tracing、runtime、引导、信号；产出二进制 {{crate_name}}
testkit/   测试共享件（只进 dev-dependencies）
reconcile/ 期望集 reconcile 框架（可选，app 默认不依赖）
```

依赖只允许向下：`app → {api, worker, storage} → core`；`api` 与 `worker` 互不依赖，
衔接只经 core 里的共享态与事件。新增依赖边要在评审说明。

## 运行

```bash
cargo run -p {{crate_prefix}}-app                 # 缺配置时落盘 <安装根>/config/{{crate_name}}.toml
cargo run -p {{crate_prefix}}-app -- --config /etc/{{crate_name}}/{{crate_name}}.toml
{{env_prefix}}_CONFIG=/etc/{{crate_name}}/{{crate_name}}.toml cargo run -p {{crate_prefix}}-app
```

- **安装根 = 可执行文件所在目录**，缺省布局是它下面两个同级目录：
  `config/{{crate_name}}.toml` 与 `data/{{crate_name}}.db`。锚在二进制而不是进程 cwd——
  systemd 拉起的服务 cwd 通常是 `/`。
- 配置路径取值顺序：`--config` > `${{env_prefix}}_CONFIG` > 缺省布局。前两者**只改配置文件
  位置，不动安装根**：单元文件里把缺省路径显式写出来不该让 `data/` 换个地方落。
- 日志出 compact 单行（stdout），`RUST_LOG` 按 crate 过滤：`RUST_LOG={{crate_prefix_snake}}_worker=debug`。
- 配置里任何字符串值都可以写环境变量占位符：`${env.NAME}`（必须设置）或
  `${env.NAME:默认值}`；相对路径相对**安装根**（SQLite 库留空即落在 `data/` 下）。
- `kill -HUP <pid>` 热重载：可热字段下一 tick 生效，`enabled` 类启动闸只在启动时读一次，
  `[http]` / `[runtime]` 段改了会回滚为运行值并在日志里列出 `requires_restart`。
- `SIGINT` / `SIGTERM`：广播 `ShuttingDown` → 根令牌取消 → 10s 内收割全部顶层任务，
  超时的强制 abort → 存储关闭 → runtime 收尾。任一顶层任务自行退出也会触发同一序列（first-failure）。

## 端点

| 路径                     | 响应                                                               |
| ------------------------ | ------------------------------------------------------------------ |
| `GET /v1/service/health` | liveness：进程活着即 200，不查依赖；成功信封                       |
| `GET /v1/service/ready`  | readiness：存储自检通过才 200，否则 503 信封                       |
| `GET /v1/service/info`   | 构建串、启动时刻、各面指标快照；裸 JSON                            |
| `GET /v1/**` 未命中      | JSON 信封 404：`{"status":"error","code":404,"description":"..."}` |

所有端点都在 `/v1` 下，探针也不例外：版本面之外再留一片无版本路径，等于给自己开第二套
契约。探针路径本来就是编排器配置里的一个字符串，跟着版本走没有额外成本。

三条响应契约由 `api/src/response.rs` 的类型保证，加端点时照抄返回类型
`Result<ApiResponse<T>, HttpError>` 就落在契约里：成功带数据回**裸 JSON**（不套包装），
成功无数据回 `{status: "success", code: 200, description: ""}`，任何错误回同形信封
（`status: "error"`，`code` 等于 HTTP 状态码）。

**取参用 `api::extract` 下的 `Json` / `Query` / `Path`，不要用 `axum::` 下的同名类型。**
提取器的拒绝发生在 handler 之前，`axum::Json` 直接回一行 `text/plain`，第三条契约就在
第一个带请求体的端点上无声地破了。包装只把拒绝换成信封，状态码仍是 axum 按 RFC 判的
（400 / 413 / 415 / 422），理由见 `api/README.md`。

## 怎么加一个任务面

1. 在 `worker/`（或新建的面级 crate）里写入口 `pub fn run(deps…, cancel: CancellationToken) -> impl Future<Output = ()> + Send + 'static`：
   不要在里面 `tokio::spawn` 自己，`select!` 的第一分支是 `cancel.cancelled()`（`biased`），
   可热字段每 tick 经 `ConfigHandle::current()` 现读；业务失败只记指标与日志，不让循环退出。
   这个面要管的是**一组由配置或库表决定的长活单元**（每租户、每设备、每队列）而不是一条
   循环时，别手写调度骨架，加一条依赖边用 `reconcile/`，接法见 `reconcile/README.md`。
2. 在 `core/src/config/` 加该面的配置段（照 `cfg_ticker.rs`：`enabled` 启动闸 + 节奏参数 + `sanitize` 钳位），
   然后**五处接线一个都不能少**：

| 改哪里                            | 做什么                                                                       | 漏了会怎样                                                                                                                                                    |
| --------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `app.rs` 的 `AppConfig`           | 加字段并在文档注释里写明热度                                                 | 编译不过                                                                                                                                                      |
| `app.rs` 的 `AppConfig::sanitize` | 加一行委托                                                                   | 钳位形同虚设，越界值直接生效                                                                                                                                  |
| `app.rs` 的 `AppConfig::validate` | 面有 `runtime` 绑定就加进跨段校验的那张表                                    | 绑到未声明的 runtime 时不报错，静默回落到主 runtime                                                                                                           |
| `core/src/config/default.toml`    | 加对应的 `[<面>]` 段与逐行注释                                               | **没有任何测试会红**：`Default` 补齐了缺失段，落盘模板照样等于 `Default`。后果是这个旋钮在运维实际编辑的那个文件里根本不出现                                  |
| `store.rs` 的 `reload` 回滚块     | 面有 `runtime` 绑定就加一行 `fresh.<面>.runtime = old.<面>.runtime.clone();` | **没有任何测试会红**：`classify` 按 `.runtime` 后缀已把它判进 `requires_restart`，但 watch 里存的是**新**绑定：重载报告说「要重启才生效」，读端却已经看到新值 |

   `COLD_PREFIXES` **不用动**：它只登记整段不可热的段（`http.` / `runtime.` / `storage.`）。
   面的段是热的，只有 `.runtime` 与 `.enabled` 两个字段特殊，`classify` 已按后缀通判。
   最后两行没有编译器兜底，所以照 `store.rs` 里的
   `reload_rolls_back_only_the_binding_field_of_a_hot_section` 给自己的面也补一条回滚测试——
   这是把「漏一行」从静默变成红的唯一办法。
3. 在 `core/src/metrics.rs` 加该面的 `XxxStats` / `XxxSnapshot`，只加真实会写的字段；
   同时把快照挂进 `api/src/handler/system.rs` 的 `MetricsView`——那个结构体逐面枚举，
   漏了不报错，只是 `/v1/service/info` 从此少报一个面。
4. 在 `app/src/boot.rs::register_runtime_tasks` 里注册：插在 `http` **之前**；`enabled = false` 时不注册并 `warn!`。
5. 给 `app/src/boot.rs` 的测试加一条「关闸时少一个任务且日志有 warn」，照 `disabled_ticker_is_skipped_with_a_warning`；
   再把既有三处 `supervisor.len()` 断言各加一（两处在 `boot.rs` 的测试里、一处在 bind 失败那条）。
   这些基数钉的是「http 最后注册」的顺序不变量，不该改成动态计算。

## 怎么加一个 crate

1. 目录名不带前缀（`core/`、`api/`），包名带：`[package] name = "{{crate_prefix}}-xxx"`，`version` / `edition` / `rust-version` / `authors` 全部 `.workspace = true`。
2. 根 `Cargo.toml`：`members` 加一项；`[workspace.dependencies]` 加 `{{crate_prefix}}-xxx = { path = "xxx" }`；第三方依赖只在根表声明一次并写一句为什么。
   根表只是版本与 feature 的单一来源，**不建立依赖边**：消费方还要在自己的 `[dependencies]` 里写 `{{crate_prefix}}-xxx = { workspace = true }`，并在评审说明这条新边。
3. 只允许向下依赖；`lib.rs` 只做 `mod` 声明与显式 `pub use`，子模块默认私有；不初始化 tracing、不 `tokio::spawn` 常驻任务。
4. crate 级 README 四段：边界 / 目录 / 关键决策 / 测试形态。

## 怎么换存储后端

改配置，不改代码：

```toml
[storage]
backend = "postgres"

[storage.postgres]
host = "db.internal"
dbname = "{{crate_name}}"
user = "{{crate_name}}"
password_env = "{{env_prefix}}_PG_PASSWORD"   # 或 password_file = "/run/secrets/pg"
sslmode = "require"
```

目标库须已存在（不自动建库）。`[storage]` 整段不可热重载。上层代码只见
`Arc<dyn Storage>`，看不见连接池，换后端不波及任何调用方——这也是「绝不允许单后端落地」
纪律的用处所在：两个后端始终等价。

## 怎么加一个仓储

按垂直切片，一次一个真实用例，八步顺序固定（详见 `storage/README.md`）：

1. 明确用例（有上层调用方才动手，不预铺）；
2. 在 `storage/src/model.rs` 定义行模型；
3. 在 `storage/src/repo.rs` 定义仓储 trait（方法按用例来，不按 CRUD 全集来），在 `Storage` 上加访问器 `fn xxx(&self) -> &dyn XxxRepo`；
4. **成对**新增 `storage/migrations/{sqlite,postgres}/<NNNN>_<name>.sql`（取当时的下一个号，不预留）；
5. 抽出两后端共用的 SQL 常量，方言差异处再分叉；
6. 实现两个后端（SQLite 读走只读池、写走写池；驱动错误归一化为 `StorageError` 的语义变体）；
7. 补双后端契约测试：断言写在一个共用函数里跑两次，**不要两个后端各写一份**——两份迟早在细节上分家，
   而「两后端等价」正是这套测试唯一要证明的事。PG 由 `{{env_prefix}}_TEST_PG_HOST` 门控，声明了连不上要红而不跳。
   同一提交里还要改三处既有断言（漏了当场红，不会静默）：

| 改哪里                                                                   | 改成什么                                                       |
| ------------------------------------------------------------------------ | -------------------------------------------------------------- |
| `sqlite_bootstrap.rs` 的 `migration_version`                             | 新的版本号                                                     |
| `postgres_bootstrap.rs` 的 `migration_version`                           | 同一个版本号                                                   |
| `sqlite_bootstrap.rs::migration_set_creates_no_business_tables` 的表清单 | 加上新表名（该用例钉的是「不预建表」，清单随每个切片显式增补） |

   PG 侧是**共**实例、SQLite 侧是一用例一库，所以断言里的业务数据要带一个每次运行都不同的
   后缀（如 pid），否则第二次跑就会撞上上次留下的行——唯一约束那条断言会在错误的地方变绿。
   门面刻意不提供 TRUNCATE，隔离只能落在数据上。
8. 最后在上层接线：handler 里 `?` 即可，`api/src/error.rs` 的 `From<StorageError> for HttpError` 已把 404 / 409 / 500 映射好。
   端点的黑盒测试断言的正是这一跳（哪个语义变体回哪个状态码），不是仓储本身——那已经在第 7 步测过了。

## 怎么给一个面单开 runtime

两处 TOML，零行代码：

```toml
[runtime.extra.compute]      # 一个附加 runtime；名字随意
worker_threads = 2           # 写 1 就是单线程隔离
max_blocking_threads = 8

[ticker]
runtime = "compute"          # 把这个面绑上去；缺省 = 主 runtime
```

- 绑定字段与 `[runtime]` 段都不可热；写了未声明的名字，配置加载期就报错，不会静默回落。
- 日志的 `threadName` 字段（`compute-0`）能看出一条日志来自哪个 runtime；关停日志的顺序是
  「全部面 stopped → storage closed → 附加 runtime 逆序 stopped → main runtime stopped」。
- 全进程只有一个 `TaskSupervisor`：`JoinHandle` 不绑定 runtime，first-failure 是进程级语义。

**准入清单——没量过就不开。** I/O 密集的面（HTTP、数据库、消息队列、定时轮询）一律不值得，
再开一个只会多一组空转线程。只有下面三种情况之一才值得：

| 症状                                                                              | 为什么单开有效                        | 先试更便宜的                                           |
| --------------------------------------------------------------------------------- | ------------------------------------- | ------------------------------------------------------ |
| CPU 密集的 async 循环拖高别的面的延迟（协作式调度无法抢占不 `await` 的任务）      | 独立 worker 池，坏邻居只饿死自己      | `spawn_blocking` 包住计算段，或每 N 次迭代 `yield_now` |
| 阻塞型 FFI / 同步 SDK 打满 `max_blocking_threads`，别的面的 `spawn_blocking` 排队 | blocking 池预算按 runtime 分          | 调大主 runtime 的 `max_blocking_threads`               |
| 某个面有独立的 P99 目标，不能受批处理面拖累                                       | 独立线程预算 = 独立调度队列与 IO 驱动 | 给批处理面加 `Semaphore` 并发上限                      |

附加 runtime 只有多线程一种形态，**没有 `current_thread`**：模板里没有任何线程会对附加
runtime 调 `block_on`，而 `current_thread` 的 runtime 要有人 `block_on` 才轮询，光
`Handle::spawn` 进去的任务永不执行、`await` 它就是挂死。要单线程隔离写 `worker_threads = 1`。

三条跨 runtime 规则（写在 `app/src/rt.rs` 模块头）：通道与令牌天然跨 runtime；IO 资源随
创建时的 runtime 注册，共享资源只在主 runtime 创建且主最后关；`spawn_blocking` 的预算按
runtime 分，不得在 runtime 线程上 `block_on`。
