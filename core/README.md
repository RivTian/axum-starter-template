# `core` — 共享词汇与进程面原语

这是 workspace 的**叶子层**：它不依赖任何其他成员，其余每一层都依赖它。

它的全部职责是给上面几层一套**共同的名字**——一个任务叫什么、一次关停分几段、一份配置的哪些字段能热改、一个存储错误分几类。名字统一之后，跨层的报告才能拼成一句话；名字不统一，每一层都会长出自己的一套同义词，而"同一件事在日志里有三种说法"是回溯事故时最费时间的一类问题。

## 它刻意不做的事

这四条各自对应一类具体失效：

| 不做 | 为什么 |
| --- | --- |
| 不装 `tracing` subscriber | 进程里只有一个全局 subscriber 槽位。一个被别人 `use` 的库有权产生事件，无权决定事件去哪 |
| 构造函数不 `spawn` | 一个悄悄起了后台任务的构造函数，会让"关停时每个任务都有归属"从第一天起就不成立 |
| 公共 API 里不出现 `sqlx::` / `axum::` / `tower::` | 门面一旦泄漏驱动类型，换实现就要改调用方——那门面就白立了 |
| 没有全局可变状态、没有隐式 `Handle::current()` | 需要什么就从参数传进来。隐式取当前 runtime 会让"这个任务跑在哪"在测试里不可控 |

它也**不读文件、不读环境变量、不解析 TOML**。配置模块只定义类型与管线的纯函数部分，真实的进程边界全在 `app`。代价是 `app` 多写一层适配；换来的是这一层整体可以在内存里验完。

## 模块

| 模块 | 内容 |
| --- | --- |
| `build_info` | 版本号的唯一来源 |
| `config` | 配置类型、变量展开、热度分类、三段式管线的纯函数部分、发布与读取、敏感取值 |
| `lifecycle` | 生命周期阶段广播（`Starting → Running → Draining → …`） |
| `paths` | 安装根锚点的纯函数 |
| `shutdown` | 关停预算：一个绝对 deadline 沿链传递的那套计算 |
| `storage` | 存储门面 `trait Storage` 与归一化错误（实现在 `storage` crate） |
| `task` | 任务面：谁在跑、怎么退的 |

## 公共出口

这份清单与源码里的公共项**两侧集合相等**，由 `make check` 的 `test` 门禁比对（实现在 `testkit/tests/discipline.rs`）。
新增一个导出而不改这里就是红的——于是"这个 crate 对外露了什么"永远有人看过。

下面那对 `exports:begin` / `exports:end` 注释不是装饰：门禁只读这两行之间的内容，
一行一个名字。它们在渲染后的 Markdown 里不可见，但改这份清单的人应该看得见它们——
清单之外的散文里也有反引号包着的标识符，让扫描器去猜哪些算数只会让它悄悄失效。
标记内的三级标题对应源码里的 `pub mod`，同样参与比对——本 crate 是六个成员里唯一有公开模块的。

<!-- exports:begin -->

### `build_info`

- `BuildInfo`

### `config`

- `Accepted`
- `ClampRecord`
- `Config`
- `ConfigError`
- `ConfigPublisher`
- `ConfigReader`
- `ConfigSnapshot`
- `ExpandError`
- `FieldPath`
- `Generation`
- `HeatDiff`
- `HttpConfig`
- `LogFormat`
- `MAIN_RUNTIME_NAME`
- `ReloadDecision`
- `RuntimeConfig`
- `RuntimeThreads`
- `SafeMessage`
- `Secret`
- `SecretLookup`
- `SecretSource`
- `StorageConfig`
- `TelemetryConfig`
- `VarSource`
- `WorkerConfig`
- `evaluate_reload`
- `expand`

### `lifecycle`

- `LifecyclePublisher`
- `LifecycleReader`
- `Phase`

### `paths`

- `config_dir`
- `data_dir`
- `default_config_path`
- `install_root_from`
- `resolve_against_root`

### `shutdown`

- `Budgets`
- `Plan`
- `Stage`
- `UnreapedTask`

### `storage`

- `CloseOutcome`
- `MigrationStage`
- `Storage`
- `StorageError`
- `StorageFuture`

### `task`

- `AckReceiver`
- `AckSender`
- `ExitKind`
- `HarvestOutcome`
- `PanicSummary`
- `PlaneError`
- `PlaneFuture`
- `RegisterError`
- `RuntimeId`
- `ShutdownClass`
- `TaskExit`
- `TaskName`
- `TaskSpec`
- `TaskSupervisor`
- `ack_channel`

<!-- exports:end -->

## 几个不显眼但重要的形状

- **回执不带负载，而且面名写在接收端。** `ack_channel(TaskName)` 造出来的一对句柄里，名字由**装配层**填、面只能发一个空信号。面拿不到写名字的机会，"任务是 A、回执记成 B"因此在类型上就不成立。一个容易想到的做法是让 HTTP 面用回执回传真实 `SocketAddr`，这里没有这么做——`bind()` 被单独列成 `api` 的出口，`app` 在 `launch()` 之前就同步 bind 完，地址从 `TcpListener` 上直接读得到。
- **`TaskSpec.class` 把"值得等"和"直接砍"分开。** `ShutdownClass::Graceful` 的面在 `harvest` 段有自己的预算，`Abortable` 的面一分钟都不等。把它做成类型而不是约定，是因为约定在第三个面加进来时就会破。
- **`reap` 有自己的绝对 deadline。** `abort()` 只能取消停在 `.await` 上的任务。一个陷在同步循环里的任务不会因为被 abort 就返回，无界地 `join_next` 等于把进程钉死在关停路径上。
- **`Config::finalize` 是管线的唯一入口。** 四步有严格顺序（锚定 → 补全 → 校验 → 钳位），顺序写错会让合法配置被判非法、或让该拦的配置被钳成"能跑但不对"。只给一个入口，调用方就没有把顺序写错的机会。
- **`evaluate_reload` 返回 `Accepted`，而 `ConfigPublisher::publish` 只收 `Accepted`。** "冷字段变了不能热重载"因此是**类型事实**，不是一条需要每个调用点各自遵守的规矩。
- **`ConfigError` 与 `StorageError` 都不携带取值**。错误里只有字段路径和一条 `&'static str` 的理由。配置里可能有口令，而错误会进日志。
- **`MAIN_RUNTIME_NAME` 是公开的常量，不是私有的字面量。** 「主 runtime 叫 `"main"`」这件事在两层里各用一次：配置管线用它判断 `http.runtime` 指的是不是主 runtime，`app` 用它给主 runtime 命名并在解析时回查。两边各写一个 `"main"` 能编译、能通过全部测试，然后在某一天有人把其中一个改成 `"primary"` 时，症状是 `http.runtime = "primary"` 静默落到了别的 runtime 上。名字有两个使用者，就必须只有一个定义点。
- **`RuntimeThreads` 出现在出口里，是因为线程数是一个有两个字段的东西。** `app` 造 runtime 时要的不是一个数字，而是「工作线程数 + 阻塞线程池上限」这一对；把它们拆成两个 `usize` 参数传过去，调用点就有了把顺序写反的机会，而写反了照样编译、照样跑。

## 测试

全部是同 crate 的 `#[cfg(test)]`——这一层没有"黑盒契约"可言，它就是一堆纯函数和内存对象，从外面测只会逼出代理断言。

```bash
cargo test -p <你的项目名>-core
```

`unwrap` / `expect` / `panic!` 在测试里是允许的，由根目录 `clippy.toml` 的 `allow-*-in-tests` 放行——那是 lint **配置**，作用域由结构性判定（`#[cfg(test)]` 模块内，或 `#[test]` 函数体内），比在每个 `mod tests` 上挂一个 `#[allow]` 更难写错。

这条豁免**不覆盖 `tests/` 下的辅助函数**：集成测试文件既不在 `#[cfg(test)]` 模块里，辅助函数也不是 `#[test]` 函数，两条判据都不占。所以夹具里要表达"这不该发生"时，返回值交回 `#[test]` 去断言，不要就地 `panic!`（实测见 `worker/tests/ticker.rs` 的 `republish_interval`）。
