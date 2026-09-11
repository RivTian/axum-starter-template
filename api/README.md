# {{crate_prefix}}-api

HTTP 展示层。无业务逻辑，只做展示与装配；数据一律经 `AppState` 注入的门面获取。

## 边界

- 依赖方向单向：`api -> core + storage`。**不依赖 worker**——两个面的衔接只经 core 里
  的共享态与事件。
- **不落库细节**：拿到的是 `Arc<dyn Storage>`，看不见连接池，因而不可能绕过门面发 SQL。
- **不建常驻任务**：`serve` 返回 future，由 app 决定 spawn 到哪个 runtime 并注册进监管器。
- **不初始化 tracing**。
- **窄门面（冻结语义）**：对外只有 `build_router` / `bind` / `serve` / `AppState` 四样。
  `handler` / `error` / `response` / `extract` 模块一律不出 crate——外部只能经 HTTP 调用
  端点，响应契约由 crate 内单测钉死，不构成代码级 API。

## 目录

```text
src/
  lib.rs            窄门面：build_router / bind / serve + JSON 404 兜底
  state.rs          AppState：全注入、零全局态，字段按真实消费者进场
  response.rs       ApiResponse：两条成功响应契约 + 共用的 GenericResponse 信封
  error.rs          HttpError：状态码集中地 + 错误信封 + 存储语义映射（按错误链还原）
  extract.rs        Json / Query / Path 包装：提取器的拒绝也回信封（加 handler 用这里的）
  handler/
    mod.rs          分组声明（crate 外不可见），每组给一个 pub(crate) router()
    system.rs       /v1/service/health、/v1/service/ready、/v1/service/info
tests/
  common/mod.rs     oneshot 黑盒脚手架（不 bind 端口）
  system.rs         系统组黑盒：探活、自省、/v1 兜底
```

加一组端点 = 加一个 handler 子模块 + 在 `build_router` 里补一行 merge，既有组不动。

## 关键决策

**bind 与 serve 分开，是多 runtime 逼出来的。** tokio 的 IO 资源在**创建时**注册到当前
runtime 的驱动，而 HTTP 面可能被绑到附加 runtime 上。所以同步 `std::net` bind 留在启动
期（端口被占、地址非法原样上抛，fail-fast），`from_std` 挪进 `serve` 的 future 体内，
注册到的才是被 spawn 到的那个 runtime。

**装配顺序不是随手排的**（axum 0.8 路由语义）：

- `v1.fallback(json_404)`：被嵌套的 Router 自带 fallback 时，外层 fallback 不再被继承——
  `/v1/**` 未命中必须是 JSON 信封，不能掉进 axum 默认的空 404；
- `route("/v1/", …)`：嵌在 `/v1` 的 Router 匹配 `/v1` 而**不匹配** `/v1/`，末端通配也不
  匹配空串，这一个路径两头都不中，要显式按回 `json_404`。前两条由黑盒测试同时打
  `/v1`、`/v1/`、`/v1/nope` 钉死；
- layer 后加的在外层：请求先过 `TraceLayer` 再过 `TimeoutLayer` 再进路由，超时也被 trace 记到。

**状态码只写在一处。** 所有对外错误都走 `HttpError`，包括 404 兜底——另起炉灶拼响应体，
两边迟早跑偏。`storage_semantics` 已把 `NotFound` → 404、
`VersionConflict` / `UniqueViolation` / `Conflict` → 409、其余 → 500 映射好，仓储进场后
handler 里直接 `?` 即可，不必各自翻译。

**存储语义按错误链还原，不只看顶层类型。** 仓储调用隔着一层返回 `AppResult` 的领域函数时，
`?` 会把 `StorageError` 升格成 `AppError::Storage(anyhow)`；只 match 顶层的话 404 / 409 会
**静默退化成 500**。`From<AppError>` 因此沿 `source()` 逐层 `downcast_ref` 找存储根因
（逐层是为了容纳领域层加过的 `anyhow::Context`）。两组单测钉死这条，别改回 `#[from]`。

**内部错误的日志与文案分离。** `Internal` 对外只回泛化文案（原文可能带路径、连接串），
原文在错误映射处记一条 `tracing::error!`——集中一处，handler 里不用各记各的。

**三条响应契约，由类型而不是约定保证。**

| 场景         | 类型                | 响应体                                            |
| ------------ | ------------------- | ------------------------------------------------- |
| 成功、有数据 | `ApiResponse::Data` | **裸 JSON**，业务对象本身，不套外层包装           |
| 成功、无数据 | `ApiResponse::Ok`   | `{status: "success", code: 200, description: ""}` |
| 任何错误     | `HttpError`         | 同形信封，`status: "error"`，`code` 等于状态码    |

有数据就裸给，是因为成功路径上那个恒为 200 的 `code` 不带任何信息——状态码已经说过
一遍了，多包一层每个消费者都要多剥一次。无数据时反过来：裸给等于回一个空 body，
客户端分不清「成功但没东西」与「响应被截断」。两个信封共用 `response.rs` 里同一个
`GenericResponse` 结构体，因而不可能长歪成两种形状；handler 照抄返回类型
`Result<ApiResponse<T>, HttpError>` 就落在契约里。

**取参一律用 `extract` 下的 `Json` / `Query` / `Path`，不要用 `axum::` 下的同名类型。**
第三条契约在提取器这里有个洞：解析失败发生在 handler **被调用之前**，`axum::Json` 的拒绝
直接就是一行 `text/plain`，客户端拿去 `JSON.parse` 报的是语法错误——而真正的原因是
「你少了个逗号」。模板自带的三个端点都不取参，所以这个洞今天是关着的，会在**第一个**
带请求体的 handler 上无声地打开：没有一行代码改动，也没有一条测试会红。`extract` 里的
三个包装只改一件事——把拒绝换成信封，状态码与文案原样留：

- 状态码取 `rejection.status()`，**不**一律拍成 400。axum 已按 RFC 分好：体不是 JSON →
  400，是 JSON 但形状对不上 → 422，`Content-Type` 不对 → 415，体过大 → 413。拍平会让
  调用方分不出「格式发错了」与「少了个字段」。
- 文案取 `rejection.body_text()`，那就是 axum 本来要写进 body 的那段字：包一层只换外壳、
  不换诊断。
- 提取器报 5xx（`Path` 取的参数个数与路由对不上等）说的是本服务的 bug，由 `HttpError`
  按内部错误处理：原文进日志、对外泛化，与 `Internal` 同一条路。

**所有端点都在 `/v1` 下，探针也不例外。** `/v1/service/health` 进程活着即 200 且不查
依赖；`/v1/service/ready` 存储自检通过才 200，否则 503 信封。版本面之外再留一片无版本
路径，等于给自己开第二套契约（第二个未命中兜底、第二处响应形状），而探针路径本来就是
编排器配置里的一个字符串，跟着版本走没有额外成本。

**路由路径只写在这个 crate 里。** 别的 crate 的注释提到端点时写**角色**不写路径——
「就绪探针的判据」而不是「`/v1/service/ready` 的判据」。路径是 `api` 的实现细节，
角色才是它想说的那件事；把路径抄进 `core` / `storage` 的注释里，改一次路由就得跨 crate
grep 一遍，而漏掉的那几处不会让任何测试变红，只会慢慢变成假话。例外是 app 里那两条
打真 TCP 的测试：它们**必须**知道路径，而写错了会当场失败，不是静默腐烂。

**两个 layer 不建 middleware 目录。** `TraceLayer` 与 `TimeoutLayer` 内联在路由装配处，
出现第三个真实 layer 再谈目录。超时回 408 而不是 503：客户端能分辨「服务没答」与
「服务答了个错」。

**`AppState` 只有读端。** 配置字段是 `ConfigHandle` 而非 `ConfigStore`——写端唯一的合法
消费者是将来的配置 reload 端点，那时再加，防止它被随手拿来改配置。

## 测试形态

黑盒为主：`tests/` 里用 `build_router` + `tower::ServiceExt::oneshot` 发真 HTTP 请求，
断言状态码与响应体。真路由、真提取器、真序列化都在被测面里，且**不 bind 端口**——
没有 TIME_WAIT，也没有并行测试抢端口。handler 模块是私有的，这也是外部消费端点的
唯一方式。

`src/error.rs` 里另有三条单测直接构造 `HttpError` 断信封与状态码：映射表值得就地钉死，
不必每加一个变体就去加一条端点。`src/response.rs` 同理，三条契约各一条单测，其中
「裸 JSON」那条的判据是信封的三个键一个都不在——只断言业务字段在的话，套了信封也照样过。

`src/extract.rs` 的单测自带一个两端点的小 Router：模板里没有取参的真端点可借，而这几条
要打的正是**请求根本进不了 handler** 的那条路径。四种拒绝（语法坏 400、形状不对 422、
`Content-Type` 不对 415、查询串坏 400）各断一次状态码与信封，外加一条 5xx 不泄漏原文。

每个集成测试二进制各自编译 `common/mod.rs`，未用到的助手在别的二进制里是死代码，
属脚手架常态。

```sh
cargo test -p {{crate_prefix}}-api --all-targets
```
