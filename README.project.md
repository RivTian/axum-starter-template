# {{crate_prefix}}

一个 Rust 后端服务的骨架：axum + tokio + SQLite，六个 crate 单向分层，一条从进程启动到干净关停
的完整路径。业务逻辑是空的——**那部分留给你**，其余部分已经写完并且有用例盯着。

它解决的不是"怎么写一个 handler"，而是那些写第三个 handler 之前不会疼、写到第三十个的时候
一次性全部发作的东西：配置从哪来、日志去哪、Ctrl-C 之后在途请求怎么办、哪一层可以依赖哪一层、
以及"这些规矩由谁来守"。骨架里每一条纪律都配一个具体失效场景，和一个会判红的落点。

---

## 跑起来

```bash
make dev-config   # 把配置文件放到开发期该在的位置
make run          # 起服务（= cargo run）
```

另一个终端：

```bash
curl localhost:8080/healthz          # {"live":true}
curl localhost:8080/readyz           # {"ready":true}
curl localhost:8080/v1/info          # {"service":"{{crate_prefix}}","version":"0.1.0","uptime_seconds":12}
```

回到第一个终端按 Ctrl-C：

```text
lifecycle phase changed from="running" to="draining"
shutdown sequence started phase="draining" plan_invalid=false
a plane exited task="ticker" runtime=1 kind="returned"
a plane exited task="http"   runtime=1 kind="returned"
the storage pool was closed outcome="closed"
runtime stopped runtime=main budget_ms=24998 elapsed_ms=0
```

退出码 0。**每个在册的面都有一条闭合记录**是这份骨架的核心断言之一：一次关停要么每个面都交代了
自己是怎么退的，要么这次关停就不算干净、退出码变成 1。少一条 `a plane exited` 不会被当成"大概
没事"。

### 配置和数据在 `target/debug/` 下，这是有意的

安装根 = **可执行文件所在的目录**，不是当前工作目录，也不是仓库根。于是：

```text
target/debug/                    ← 开发期的安装根
├── {{crate_prefix}}
├── config/service.toml
└── data/service.sqlite3
```

换个目录启动进程，读到的是同一份配置、连的是同一个库。这条性质不靠"试几个目录看结果一样"来
保证——`std::env::current_dir` 在整个 workspace 里**一次都不出现**，门禁逐字节扫这件事。cwd 根本
没有进入任何决策，也就不需要枚举它的取值。

代价就是第一次的直觉冲突：`cargo clean` 会把你的开发配置和开发数据一起删掉；`cargo run --release`
的安装根是 `target/release/`，那是**另一份**配置、**另一个**数据库。`make dev-config` 会把这两句话
再说一遍。

真正部署时把可执行文件放进 `<安装根>/`，`config/` 和 `data/` 与它同级。这两个目录刻意不是父子
关系：一个运维改、一个进程写，权限和备份策略不一样。

---

## 目录

```text
core/       共享词汇：任务名、关停预算、配置类型、存储门面。不依赖任何其他成员
storage/    SQLite 实现。门面在 core，这里只有实现
worker/     周期任务面的样板
api/        HTTP 面。**你的路由写在 api/src/business.rs**
app/        装配层。唯一有 main 的 crate
testkit/    测试夹具 + 结构纪律门禁（dev-only，进不了二进制）
```

依赖只能沿一个方向走：

| from \ to | core | storage | worker | api | app |
| --- | --- | --- | --- | --- | --- |
| `core` | — | ✗ | ✗ | ✗ | ✗ |
| `storage` | ✔ | — | ✗ | ✗ | ✗ |
| `worker` | ✔ | ✗ | — | ✗ | ✗ |
| `api` | ✔ | ✗ | ✗ | — | ✗ |
| `app` | ✔ | ✔ | ✔ | ✔ | — |

这张表不是建议。`make check` 把它当**相等**比较：多一条边红，少一条边也红。包含比较放得过
新增边，而新增的那条边通常长得很无辜——`worker` 想直接读一下数据库，于是后台任务面从此认识
了 SQLite，换存储后端要改两个地方。

每个 crate 根下都有一份 README，写的是那一层**刻意不做什么**以及为什么。读代码之前先读它们，
能省掉一多半"这里为什么绕了一圈"的疑问。

---

## 配置

一份 TOML，三段优先级：

```text
1. 命令行      --config <PATH>
2. 环境变量    {{crate_prefix | upcase | replace: "-", "_"}}_CONFIG
3. 缺省位置    <安装根>/config/service.toml
```

只有第 3 档在文件不存在时会**创建**它（内容与程序内置默认值逐字段相等）。前两档指到一个不存在
的文件一律启动失败——显式指定了路径却被静默换成默认配置，是那种要到出事才会被发现的事。

配置项的说明写在 `app/assets/service.toml` 里，每个键旁边都有一句"改它要付什么代价"。那份文件
就是内嵌模板本身，不是它的副本。

### 热的、半热的、冷的

`SIGHUP` 触发整批重载：

```bash
kill -HUP $(pgrep -x {{crate_prefix}})
```

| 热度 | 例子 | 语义 |
| --- | --- | --- |
| 热 | `worker.tick_interval`、`http.handler_timeout` | 改了立刻生效，下一次使用时现读 |
| 半热 | `http.body_limit_bytes` | 值换了，但要等那个面重启才用上 |
| 冷 | `http.bind_addr`、`[runtime]`、`[telemetry]` | 改了必须重启进程 |

重载是**事务式**的：冷字段变了，整批退回，一个字段都不换，日志里说清是哪个键挡住的。不做"能换
的先换"——那会得到一个配置文件和进程状态对不上的进程，而排障的人只会去读配置文件。

删掉配置文件再发 SIGHUP **不会**把默认模板重建出来，重载会失败并保留上一份好配置。往磁盘写配置
的代码只在启动路径上存在一份。

---

## 日志

结构化事件，默认 text 格式写 stderr，`[telemetry].format = "json"` 换成一行一个 JSON 对象。

- 每条事件都有一个**稳定短名**（`name:`）和一句给人读的话。前者给机器过滤，后者给人读——同一条
  事件不该逼你二选一。
- 过滤器语法同 `RUST_LOG`，但进程**不读** `RUST_LOG`：真值只有 `[telemetry].filter` 一处。写错了
  会退回 `info` 并留一条 `warn`，不会静默。
- 着色只由"stderr 是不是终端"决定，与配置无关。管道里不会出现转义序列。
- 错误里**不带取值**。配置里可能有口令，而错误会进日志；`ConfigError` 和 `StorageError` 只携带
  字段路径和一条固定的理由。

---

## 关停

Ctrl-C / SIGTERM 之后是一条固定序列，四段预算来自**同一个绝对 deadline**：

```text
公告 Draining → 级联取消 → 等优雅面收尾 → abort 剩下的并回收 → 关存储 → 关 runtime
```

因为是一个 deadline 沿链传递，而不是四个独立超时，所以"每段都没超时、加起来超了"这件事不成立。

再按一次 Ctrl-C **收紧**总边界（不重新计时、不换首因），第三次立即强退。强退不会被报成一次干净
关停——退出码跟着变。

退出码只有 0 和 1。不给每一类失败分配一个专属数字：编排系统只会问"要不要重启它"，细分的后果是
有人开始在重启策略里写 `restart_on: [3, 5]`，然后这些数字就再也改不动了。详情去日志里读。

---

## 门禁

```bash
make check
```

**这一条就是全部。** 没有第二个需要你记住的入口——`check` 是 `fmt` + `lint` + `test` 三条的并集，
三条各自对应一个不同的失败类别：形态、编译器看得出但默认不拦的东西、行为**以及结构**。

后半句是这份骨架的特点：二十条结构纪律（上面那张邻接表、各 crate 的公共出口清单、`select!` 的
取消优先、`allow` 的作用域、进程边界不许有判断……）写成了普通的 Rust 集成测试，住在
`testkit/tests/discipline.rs`。它们跟着 `cargo test` 跑，所以门禁的依赖面只有 Rust 工具链本身——
不需要 `grep -P`、不需要 `sed -i`、不需要 Python，在哪台机器上都是同一个结果。

加一条新检查的方式只有一种：挂到那三条之一里面去。如果它哪条都不属于，先想清楚它到底在验什么。

两个第一天就会踩到的点：

- `lint` 和 `test` 都带 `--locked`。改完 `[workspace.package].version` 要跑 `make relock`，
  否则下一次 `make check` 会红在一个看起来毫不相干的地方。这不是缺陷，是 `--locked` 的定义。
- `cargo fmt --check` 的结果随你本机的 rustfmt 版本浮动。这份骨架**不带** `rust-toolchain.toml`
  ——替你钉死一个会过期的工具链版本不是我们该做的决定。升级 rustfmt 之后可能在没改代码的情况下
  见红，`cargo fmt --all` 一下即可。

---

## 往里加东西

四条垂直切片，覆盖绝大多数"第一次要改哪儿"的问题。

### 加一条业务路由

改 `api/src/business.rs` 的 `routes()`，一处就够：

```text
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/widgets", get(list_widgets).post(create_widget))
}
```

用 `crate::Json` / `crate::Path` / `crate::Query`，**不要**用 `axum::` 下的同名提取器：包装版在
提取失败时也走统一错误信封，原版会返回一段裸文本，于是你的 API 在 400 这一档上有两种响应形态。

信封长这样，成功响应里这三个键一个都不会出现：

```text
{ "error": "not_found", "message": "no route matches this path", "status": 404 }
```

**这里不能写 `.fallback`**，而且写了不会报错——axum 0.8.9 会先把你的 fallback 收下、再被主路由树
覆盖掉，没有 panic、没有告警。所以这条禁令由门禁扫描守着。路由树的补集本来就该统一兜底，那样
"路径不存在"和"路径存在但方法不对"才是同一套信封。

不要在 `api` 之外另起一个 `Router` 再合并到最外层：你会得到一条绕过超时、绕过 body 上限、绕过
统一信封的路由，而这种洞通常要到灰度上线才被发现。

### 加一个配置项

```text
1. core/src/config/     加字段（带 serde 默认值）
2. core/src/config/heat.rs  在热度分类里给它定性——热 / 半热 / 冷
3. app/assets/service.toml  加一个注释掉的键，旁边写清它的代价
4. 校验与钳位写进 Config::finalize 的对应步骤，不要在使用点各写一遍
```

第 2 步没有默认答案：忘了给它定性，编译器会提醒你（那个 `match` 没有 `_` 臂）。

### 加一张表 / 一个存储方法

```text
1. storage/migrations/0002_xxx.sql   新增，不改已应用的迁移
2. core/src/storage/                 在 trait Storage 上加方法（门面，不能出现 sqlx 类型）
3. storage/src/sqlite.rs             实现它
4. storage/src/memory.rs             内存实现也要跟上（test-utils feature）
```

第 2 步是那条线：门面签名里放不进 `sqlx` 的类型——`core` 根本不依赖任何数据库 crate，所以这不是
纪律，是编译器挡的。哪天你需要改门面才能做成一件事，那说明抽象漏了，值得停下来想一想。

已应用的迁移改了校验和会 fail-fast，不会被"顺手修好"。在没人看着的时候改数据库比启动失败危险
得多。

### 加一个后台面

```text
1. core/src/task/name.rs      TaskName 加一个变体（ALL 数组也要加，漏了有用例判红）
2. worker/src/                照 ticker.rs 抄一个模块
3. app/src/boot/assembly.rs   在 assemble 里纯构造它（不 spawn）
4. 同上，在 Assembled::launch 里 spawn_on + 配一个 ack_channel
```

`worker/src/ticker.rs` 是刻意做成样板的：一个后台面有一组要么全对要么全错的形状——返回 future
而不自己 spawn、`select!` 里取消优先、热配置每轮现读、先发回执再等提交门。照着抄，抄错的余地
很小。

提交门不用动，它数的是登记集。

---

## 当前边界

有意不做的东西也要列名——"文档宣称了没有的能力"和"有意不做却不说"是同一类缺陷的两面。

### 七项横切能力不在里面

| 不在里面 | 为什么 | 要加的话在哪加 |
| --- | --- | --- |
| CORS | 允许哪些 origin 是**部署事实**。预置 `Any` 是安全倒退，预置空清单是零价值 | `api/src/router.rs` 的 layer 链上加一层 `CorsLayer`，并开 `tower-http` 的 `cors` feature |
| OpenAPI | 需要在每个 handler 上挂宏属性，而这里一个业务 handler 都没有；且会把一个大依赖钉进你的项目 | `api`，连同一条 `/openapi.json` 路由 |
| Prometheus / metrics | 指标后端是部署事实。`/v1/info` 已经给了 build 与 uptime 这类零依赖的进程事实 | `api` 加一层中间件 + 一条 `/metrics`。顺带回答"谁能访问它" |
| HTTP 客户端 | 骨架里没有消费者。没有消费者的东西不预铺——预铺出来的形状总是照着想象长的，第一个真用户来的时候要推倒重来 | 照 `storage` 的样子：trait 放 `core`，实现放一个新成员，`app` 注入。记得同步更新上面那张邻接表 |
| 文件日志 | 日志目的地是部署事实：容器和 systemd 都从 stdout 收 | `app/src/telemetry.rs`。先问清楚谁来轮转、谁来清理 |
| 配置重载 HTTP 端点 | 做成外部端点就要回答鉴权，而鉴权方案是部署事实。SIGHUP 覆盖了同一个需求且不新增攻击面 | 不建议。真要做在 `api`，鉴权先行 |
| 认证 / 鉴权 | 同上 | `api`。**注意别用 `.layer()`**——系统端点和业务路由走的是同一棵树，那样会把 `/healthz` 一起挡住，探针立刻全红。对 `business` 那棵子树单独挂 |

### 平台

**只支持 unix。** 非 unix 平台是一条 `compile_error!`，不是一个没验证过的实现——声称一个没跑过的
平台比不支持它更糟。补齐很便宜：`app::run` 收的是 `impl Stream<Item = ProcessSignal>`，换掉
`app/src/signals.rs` 里那个产生器就行，编排层一行都不用动。

### 几条具体限制

- **改环境变量要重启，SIGHUP 不管用。** 进程在启动时就把"读哪个文件"和"环境变量是什么"快照冻住，
  运行期不再重算。重算要么得到同一个答案，要么意味着有人在运行期改了进程环境——而 `set_var` 在
  Rust 2024 里是 `unsafe`，这份骨架一次都没用过它。
- **半热字段只有"下次面重启生效"这一条证据。** 报告会如实列出来，但跨重启的行为没有自动化覆盖。
- **`CatchPanicLayer` 不覆盖流式 body 中途的 panic。** 响应头发出去之后再 panic，连接仍然会断。
  不宣称覆盖。
- **`spawn_blocking` 一旦开始就不可中断**（这是 tokio 的语义，不是这里的选择）。你在面里用了它，
  关停时那段工作只能被报成 unreaped。骨架自己的配置读取因此是同步有界读，不走 `spawn_blocking`。
- **存储里没有任何业务表。** `0001_baseline.sql` 是空的，门面上只有 `health()`。有一条用例盯着
  "迁移集建不出任何业务表"——你加第一张表时它会红，改它的人就被迫重新想一遍这张表属不属于骨架。
- **关停预算的上界是逻辑时钟上的。** 机器过载时真实偏差不在覆盖面内，报告会如实写 unreaped。

---

## 想读得更细

| 想知道 | 去读 |
| --- | --- |
| 每一层刻意不做什么 | 各 crate 根下的 `README.md` |
| 启动顺序为什么是那个顺序 | `app/README.md` |
| 一条纪律具体由什么守着 | `testkit/tests/discipline.rs`，每个用例名字就是它的断言 |
| 某个配置项改了要付什么代价 | `app/assets/service.toml` 的注释 |
| 为什么依赖是这几个 | 根 `Cargo.toml`，每条依赖旁边都写了理由 |
