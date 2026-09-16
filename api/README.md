# `{{crate_prefix}}-api`

HTTP 展示层。全部工作可以压成一句话：**把进程内部的东西翻译成 HTTP，并且只用一种说法。**

"只用一种说法"是这一层唯一真正难守的纪律。一个服务里错误响应有两种形状（一种是 JSON 信封、
一种是框架默认的 `text/plain`），客户端就得写两套解析——而它们会在最难复现的路径上冒出来：
路由不存在、方法不对、body 超限、handler panic、handler 超时。这五条全都**不经过任何业务
代码**，也就全都不会出现在业务测试里。这个 crate 的结构基本上是围着那五条展开的。

## 它刻意不做的事

- **不碰 `storage`。** 邻接表禁止 `api → storage`，**连 dev 边都没有**。这一层持有的是
  `Arc<dyn core::Storage>`，而 `core` 不依赖 sqlx——于是"门面不泄漏后端"由依赖图强制，不靠人
  自觉。代价是契约测试要自己写一个 `Storage` 替身；收益是那个替身能**按需失败**、还能在探测
  进行到一半时改变进程阶段，比真后端好用。
- **不装 subscriber、不建 runtime、不 spawn。** 同 `worker`。`HttpPlane::build`
  返回一个 `PlaneFuture`，交给谁去跑是 `app` 的决定。
- **不读配置文件、不读环境变量。** 配置从 `AppState` 里的读端来，那是 `app` 注入的。
- **不预铺 `From<StorageError> for HttpError`。** `From` 会被 `?` 隐式调用，于是"这个存储
  错误该回几"会散落到每个 handler 里，谁也看不见它。取而代之的是显式的
  `HttpError::from_storage`，它内部那个 `match` **没有 `_` 臂**——`StorageError` 加一个变体，
  这里当场编译不过，而不是悄悄落进一个 500。
- **不用 `axum::Json` 作为响应类型。** 它在 `clippy.toml` 的 `disallowed-types` 名单上。
  手写一层换来两样东西：content-type 是我们自己写死的，序列化失败有一条显式的兜底路径。
- **没有 `nest`。** 理由见最后一节——不是嫌它麻烦，是它会在子树上悄悄换掉 fallback。

## 公共出口

这份清单与源码里的公共项**两侧集合相等**，由 `make check` 的 `test` 门禁比对（实现在 `testkit/tests/discipline.rs`）。
本 crate 没有 feature，所以只有一份清单。下面那对 `exports:begin` / `exports:end` 注释是给门禁
读的：它只取每一行第一格里的名字，一格多个用 ` / ` 分开。标记之外的散文里也有反引号包着的
`Router`、`axum`、`AppState::new(...)`，那些都不是出口——所以边界要写出来，不能让扫描器猜。

<!-- exports:begin -->

| 名字 | 是什么 |
| --- | --- |
| `router` | 组装整棵路由树：系统端点 + 你的路由 + 两个 fallback + 四层中间件 |
| `business_routes` | **你加路由的落点**（源码里是 `business::routes`）。默认返回空路由器 |
| `AppState` | handler 能看到的一切：存储门面、生命周期读端、配置读端、构建信息、启动时刻 |
| `HttpError` | 统一错误类型。字段全私有，构造途径只有两条 |
| `ENVELOPE_KEYS` | 信封的三个键名（`error` / `message` / `status`）的**唯一**真值 |
| `Json` / `Path` / `Query` | 包装提取器。失败时回信封，不回 `text/plain` |
| `bind` | **同步**地占住端口，返回 `std::net::TcpListener` |
| `HttpPlane` | `SPEC`（`TaskName::Http` + `ShutdownClass::Graceful`）与 `build` |

<!-- exports:end -->

`business_routes` 存在的理由不是对称美感，是**装配层念不出 `Router` 这个名字**：根清单把
`axum` 圈在这一层，`app` 的依赖里没有它。如果 `router(state, business)` 的第二个参数只能
由调用方现造，那么"加第一条业务路由"这件事就得从改根清单的依赖边开始——而那条边是一整条纪律
的支点。给出一个默认值之后，`app` 写的是 `router(state, business_routes())`，类型靠推导，
`axum` 一次都不出现。

`HttpPlane` 是个纯命名空间——没有字段，也不需要被构造出来。`SPEC` 属于这一层的理由与
`TickerPlane::SPEC` 一样：「"这个面需不需要优雅收尾"只有写这个面的人知道」。这一层对外
本来只需要 `bind` 与 `build` 两个出口，`SPEC` 是后加的。

`AppState` 有一个私有的 `started: Instant`，所以只能经 `AppState::new(...)` 构造。这是刻意的：
`/v1/info` 报的 uptime 必须是**这个进程**的，让调用方用结构体字面量自己填一个，那个数字就成了
装配层随手写的东西。

## 信封

错误响应**只有一种形状**，三个键不多不少：

```text
{"error": "not_found", "message": "no route matches this path", "status": 404}
```

- `error` 是稳定短名，客户端**用它分支**。
- `message` 是给人读的一句话。4xx 可以带细节（细节本来就来自客户端自己送的东西）；
  **5xx 只能是字面量**——真实原因通常来自内部（SQL 报错、panic 负载、序列化失败），原样回给
  客户端就是一次信息泄漏。
- `status` 与 HTTP 状态码相同。冗余是故意的：不少客户端和网关会把状态码吞掉。

5xx 只能是字面量这件事**不靠"记得"维持**，靠类型：`HttpError::server` 的 `description` 参数是
`&'static str`，`HttpError::client` 的才是 `impl Into<Box<str>>`。于是"一个 `String` 出现在 5xx 的
响应体里"在类型上没有入口。

成功响应里**一个信封键名都不会出现**（模板自己的五条端点是这么写的，也有一条用例盯着）。
于是客户端可以靠"有没有 `error` 键"判断成败，不必先看状态码。

### 六个漏点，六处接管

| 漏点 | 不接管会怎样 | 这里做了什么 |
| --- | --- | --- |
| 路由不存在 | axum 回 `404` + **空 body** | 顶层 `fallback` |
| 方法不对 | axum 回 `405` + **空 body** | `method_not_allowed_fallback` |
| 提取器失败 | axum 回 `text/plain` | 包装提取器 `Json` / `Path` / `Query` |
| body 超限 | 同上（走提取器） | 同上，保住 `413` 这一档 |
| handler panic | tower-http 回 `text/plain` 的 `Service panicked` | `CatchPanicLayer::custom` |
| handler 超时 | tower-http 回 `408` + **空 body** | 自己写的中间件，回 `504` 信封 |

后两条值得单独记一笔：它们是**同一类**缺陷——第三方中间件各自带了一套"出错时回什么"，而
它们都不知道本服务有信封这回事。超时那条是实测 tower-http 时撞见的，panic 那条更隐蔽——
要等到某个 handler 真的 panic 那天才会暴露。所以这里留下一条可复用的判据：

> **凡是能自己产出响应的第三方层，都要先问它产出的是不是信封。**

加中间件时请照这条问一遍。

## 中间件顺序

axum 的规则是**后 `.layer()` 的在外面**，请求自下而上穿过。所以 `router.rs` 里那串 `.layer()`
读起来是从内到外，实际长这样：

```text
请求 ─→ TraceLayer ─→ CatchPanicLayer ─→ handler_timeout ─→ DefaultBodyLimit ─→ handler
```

| 层 | 为什么在这个位置 |
| --- | --- |
| `TraceLayer` | **必须最外**。它要记的是"这次请求发生了什么"，包括被超时拦掉的、被 panic 拦掉的、被 body 上限拒掉的。放里面的话，恰恰是最该看见的那几类请求不会出现在日志里 |
| `CatchPanicLayer` | 在超时**外面**。panic 可能发生在超时中间件自己身上。反过来放，那一类 panic 会直接穿到 hyper，连接被砍，客户端拿到的是一次 EOF 而不是 500 |
| `handler_timeout` | 在 body 上限**里面**。上限靠 extension 生效，必须在提取器跑之前装上；而超时要计的是 handler 自己花的时间 |
| `DefaultBodyLimit` | 最内。它只往请求上挂一个 extension，挂得越靠近 handler，中间被别人改掉的机会越小 |

`DefaultBodyLimit` **不需要** tower-http 的 `limit` feature：它是 axum-core 的东西，靠
`RequestExt::into_limited_body()` 走 `http_body_util::Limited`，超限的 `LengthLimitError` 被
downcast 成 413，整条路径与 tower-http 无关。根清单里那条 feature 已经删掉了。

### 有一个顺序是死的

**先路由、再 fallback、再 405、最后 layer。**

`Router::layer` 会同时作用到 `path_router`、`fallback_router` 和 `catch_all_fallback`，所以
fallback 也被信封和 trace 覆盖——**前提是它在 `.layer()` 之前就挂上了**。
`method_not_allowed_fallback` 同理，axum 的文档原话是它只改 "all previously registered" 的
`MethodRouter`。

有人把 `.route()` 挪到 `.layer()` 后面，新路由会静悄悄地不带任何中间件——**不报错、不告警**。

## 系统端点

三条端点回答**三个不同的问题**。压成一个的话，编排器就分不清"杀掉它"和"别再给它流量"。

| 端点 | 问题 | 排空期的答案 |
| --- | --- | --- |
| `GET /healthz` | 这个进程还活着吗？ | `200`——正在正常关门的进程是健康的 |
| `GET /readyz` | 现在能给它流量吗？ | `503`，`error` 是 `draining` |
| `GET /v1/info` | 这是哪一版？ | `200` |

`/healthz` **不看阶段、也不看存储**。它变红的唯一含义应当是"进程该被重启了"；让它跟着存储走，
一次数据库抖动就会把一批健康的进程杀掉重来。

`/readyz` 的顺序是**阶段 → 存储探测 → 再读一次阶段**。最后那一次不是冗余：探测是跨 `await` 的,
期间进程完全可能进入排空，而"探测开始时还能服务"不等于"探测结束时还能服务"。

`error` 短名的词表是 `Phase::as_str()` 的全集加一个 `storage_unavailable`——**不另起一套同义词**。

`/healthz` 与 `/readyz` 不带版本前缀：它们是**进程**的属性，不是 API 的一部分，给它们带上版本号
等于承诺"v2 里可能换一套存活语义"，而那不是真的。`/v1/info` 例外——它报的正是这一版 API 背后的
构建信息。

## 提取器的四档失败

包装提取器把状态码**直接取 axum 给的那一个**，不在这里重新编一张表。

| 情况 | 状态码 | `error` |
| --- | --- | --- |
| JSON 语法错误 | `400` | `bad_request` |
| 缺 `content-type` | `415` | `unsupported_media_type` |
| 字段不符 / 反序列化失败 | `422` | `unprocessable_entity` |
| 超过 `http.body_limit_bytes` | `413` | `payload_too_large` |

不自己抄表有两条理由，第二条才是关键的：

1. 需要区分的那几档**恰好**就是 axum 0.8.9 的默认值。抄一份只是多一份会与上游漂移的副本。
2. **axum 的分档比一张手抄表细。** `FailedToDeserializePathParams` 是手写的（不是宏生成的），
   它把「路径参数类型不符」判成 `400`，却把「handler 的 `Path<T>` 元数与路由上的参数个数
   对不上」判成 **`500`**——后者是**程序员写错了**，不是客户端发错了。手抄表几乎必然把这两种
   压平成同一个 `400`，于是一个自己写错的 bug 会被报成客户端的错误，在监控上永远查不出来。

`tests/contract.rs` 里四档各有一条用例，并断言它们**彼此不相等**（不许拍平成一个 400）；
另有一条专门跑第 2 点里那个 500，并断言细节**没有**出现在响应体里。

## 事件

**每个事件都有显式的 `name:`，名字是 snake_case 标识符；消息位置留给给人读的那句话。**
不写 `name:` 的话 `tracing` 自动生成的是 `event api/src/system.rs:90` 这种**带行号**的名字——
行号会烂，靠它做断言等于把用例钉在源码布局上。把标识符塞进消息位置（这一层最初就是这么写的）
看着更省事，实际上是把两个角色压成了一个：日志里没有人话，断言里没有稳定的键。

这一层的 target 统一是 `http`。下面这些名字是**可以被断言的**：

| 名字 | 级别 | 什么时候 | 关键字段 |
| --- | --- | --- | --- |
| `readiness_probe_failed` | `WARN` | `/readyz` 的存储探测失败 | `kind`、`error` |
| `handler_timed_out` | `WARN` | handler 超出 `http.handler_timeout` | `method`、`path`、`budget_ms` |
| `handler_panicked` | `ERROR` | handler panic 被 `CatchPanicLayer` 接住 | `panic`（负载原文） |
| `extractor_rejection_withheld` | `ERROR` | 5xx 的提取器 rejection，原文没进响应 | `status`、`code`、`detail` |
| `storage_error_withheld` | `ERROR` | 5xx 的存储错误，原因没进响应 | `code`、`error` |
| `unclassified_error` | `ERROR` | 错误链里找不到 `StorageError`，只能判 500 | `error` |
| `response_serialization_failed` | `ERROR` | `Serialize` 失败，走了兜底 body | `type_name`、`error` |
| `listener_registration_failed` | `ERROR` | `from_std` 失败（面还没回执） | `error` |
| `accept_loop_failed` | `ERROR` | accept 循环因 io 错误结束 | `error` |

名字里带 `_withheld` 的那三条是同一条原则的落点：**凡是响应里不能说的，日志里必须说。**
5xx 的 message 是固定字面量，真正的原因不在这里记，就在任何地方都记不到。

请求级的 span 由 `TraceLayer` 负责，这一层不再重复记录成功路径。

## 依赖

| 依赖 | 用到的 feature | 为什么 |
| --- | --- | --- |
| `{{crate_prefix}}-core` | — | 门面、配置、生命周期、任务面词汇。**唯一**一条成员边 |
| `axum` | 默认 | 框架。默认集是写 handler 的基本词汇，关掉只会让人在加第一个业务路由时被迫改根清单 |
| `tower-http` | `trace`, `catch-panic` | `TraceLayer` 与 `CatchPanicLayer`。**没有** `limit`，**没有** `timeout` |
| `tokio` | `time`, `net` | `timeout` 与 `TcpListener::from_std` |
| `tokio-util` | — | `CancellationToken`：`with_graceful_shutdown` 的信号源 |
| `serde` / `serde_json` | — | 响应体序列化与提取器的 `DeserializeOwned` 约束 |
| `tracing` | — | 见上一节 |

这一层**没有** `thiserror`，别的层都有。不是疏漏：`HttpError` 是个字段全私有的结构体，`Display`
要拼的是两个私有字段，派生宏在这种形状上省不下几行，却要多一条依赖。

`tower-http` 那两个没开的 feature 是同一件事的两面——**一个 feature 要么有确切的消费点，
要么不开**。`limit` 是"以为有消费点"（`DefaultBodyLimit` 根本不走它），`timeout` 是"有消费点
但那个实现是错的"（空 body、无 content-type，且 `TimeoutLayer::new` 自 0.6.7 起弃用，
在 `-D warnings` 下直接红）。`unused_crate_dependencies` 只看 crate、看不见多余的 feature，
所以这两条只能靠人删。

## 测试

```bash
cargo test -p <你的项目名>-api
```

单元测试（13 条）留给**纯函数**——状态码分档（`error.rs`）、信封渲染（`response.rs`）、panic
负载的描述（`middleware/panic.rs`）、就绪原因的词表（`system.rs`）。它们在 `src/` 里，因为测的
是私有项。外部可观察的东西全在 `tests/contract.rs`（15 条）：

| 用例 | 钉的是 |
| --- | --- |
| `the_shipped_business_router_adds_nothing` | 发布出去的 `business_routes()` 默认值真的是空的，且不影响系统端点 |
| `the_three_system_endpoints_answer_three_different_questions` | 排空期三条端点分道扬镳 |
| `readiness_names_each_way_of_not_being_ready` | `starting` / `storage_unavailable` / 就绪，三档只能靠 `error` 短名分开 |
| `a_probe_that_spans_a_phase_change_is_not_ready` | 探测**期间**相变了，这次探测的结论就不再有效 |
| `info_echoes_the_build_info_it_was_given` | 端点报的版本就是装配时交给它的那一份 |
| `success_bodies_never_use_an_envelope_key` | 成功响应里一个信封键名都没有 |
| `every_error_path_renders_the_same_three_key_envelope` | 六条错误路径同一个形状，且 `status` 与状态码一致 |
| `a_panic_payload_reaches_the_log_and_not_the_body` | panic 不杀进程；负载只进日志 |
| `a_handler_that_overruns_its_budget_is_a_504_envelope` | **504 不是 408**，且有一条带路径的记录 |
| `the_four_extractor_failures_keep_four_different_statuses` | 四档两两不等 |
| `a_query_rejection_is_an_envelope_too` | `Query` 走的是另一个 trait，不能只验 `Json` |
| `a_path_extractor_misuse_is_a_500_and_withholds_its_detail` | 委托 `.status()` 而不是手抄表的全部理由 |
| `the_spec_says_graceful_and_the_plane_owns_that_answer` | `SPEC` 属于这一层 |
| `a_taken_port_fails_at_bind_time` | 端口冲突发生在进程宣称"我起来了"**之前** |
| `the_plane_acks_then_serves_then_returns_cleanly_on_cancel` | 回执 → 真的应答 → 取消后干净返回、fd 不泄漏 |
| `the_storage_replica_is_shared_not_copied` | 夹具自身的守卫，防止上面那条 `storage_unavailable` 变成永远绿的空断言 |

几件与别处不同的事：

- **绝大多数用例走 `ServiceExt::oneshot`，不起端口。** 路由树、中间件栈、信封三样都能在内存里
  验完，而绑端口会把"端口被占"变成一类与被测内容无关的随机失败。只有两条用例需要真 socket：
  `from_std` 注册到哪个 runtime、`with_graceful_shutdown` 收到取消之后还接不接连接——这两件事
  `oneshot` 摸不到。
- **没有 HTTP 客户端。** 那条用例手写一行 HTTP/1.1 发进 socket（dev-dep 的 `tokio` 因此要
  `io-util`）。为一条用例引一个客户端 crate 不值；手写请求行反而更诚实——它证明的是"这个端口上
  真的有人在按 HTTP 应答"，与路由树的内部结构无关。
- **存储替身写在测试文件里**，不从 `storage` 借。邻接表不给 dev 边，而替身能按需失败、还能在
  探测进行到一半时把阶段推过去——真后端做不到，那正是 `/readyz` 最关键的一条断言。
- **日志断言一律是"存在"，不是"不存在"。** `LogCapture` 是进程级互斥资源，一条用例捕获期间
  并行跑的别的用例发的事件也会落进来。"某事件不存在"会被污染成假阳性；"某事件存在且带着这个
  字段"只会被污染成更容易通过。要断言"不该出现的东西没出现"，一律去看**响应体**。
- **夹具里不 `panic!`。** `clippy.toml` 的 `allow-*-in-tests` 是结构性豁免，只认 `#[cfg(test)]`
  模块和 `#[test]` 函数体；集成测试的辅助函数两样都不占。所以 `Rig::send` 返回 `Result`，
  断言留在用例里——那里本来也更适合放断言。
- 本 crate **一条 doctest 都没有**，全模板都没有：doctest 里的 `use` 必须写 crate 真名，而真名
  由模板变量展开。示例一律标 `text` 围栏，标 `ignore` 仍然会被计成一条 doctest。

## 照着加业务路由

1. 写你的 handler，用**这里的** `Json` / `Path` / `Query`，不要用 `axum::extract::*`
   （后者在 `disallowed-types` 名单上，绕过它就绕过了信封）。
2. 把它们挂进 **`src/business.rs` 的 `routes()`**——就是这一个函数，不用改装配层，也不用
   动根清单。装配层调的是 `router(state, business_routes())`，你在那里加的路由自动和系统端点
   走同一套中间件。
3. 需要存储时从 `State(state): State<AppState>` 里取 `state.storage`——那是
   `Arc<dyn core::Storage>`，你在这一层看不见 SQLite。
4. 业务错误转信封走 `HttpError::from_error_chain(&err)`：它沿 `source()` 逐层找
   `StorageError`。只匹配顶层的话，一个本该 `404` 的「不存在」会变成 `500`，而这两者在告警上
   的待遇完全不同。

### 两个会静悄悄出事的地方

**① `business` 不能带 fallback。** 直觉上这会 panic——`Router::merge` 的文档确实写了"两侧都有
fallback 不允许"。**实测不会**（axum 0.8.9）：那条 panic 只在两侧都已经是**自定义** fallback 时
触发（`axum-0.8.9/src/routing/mod.rs` 的 `(false, false)` 臂），而 `router()` 里 `.merge(business)`
排在 `.fallback(not_found)` **之前**，合并那一刻外层还是默认 fallback，走的是 `(true, false)` 臂：
你的 fallback 先被收下，紧接着被那句 `.fallback(not_found)` 覆盖掉。

结果是你的 fallback 一次都不会被调用，且没有 panic、没有告警、没有编译错误。所以这条由
`make check` 的扫描守：`src/business.rs` 里出现 `.fallback` 就红。

**② 少用 `nest`，用了就别给它加 fallback。** 内层路由器**没有**自己的 fallback 时会继承外层的,
所以 `nest` 本身是安全的。危险的是反过来：**你给 `nest` 进去的路由器加了 fallback，外层那条就
对整个子树失效了**——那一片的 404 会退回 axum 的空 body，而信封在那里悄悄消失。没有任何编译期
或启动期的信号会提醒你。

模板里因此一条 `nest` 都没有：`/v1/info` 是一条普通路由。「`nest` 内 fallback + 顶层
fallback」两处都挂看起来更稳妥，实测下来内层是多余的。
