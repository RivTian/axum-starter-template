# {{crate_prefix}}-core

叶子内核。承载所有 crate 共享、且**不含常驻任务**的内容：配置、错误基座、事件总线、
指标、顶层任务监管、rustls provider、构建元数据与时间基元。

## 边界

- **叶子**：不依赖任何兄弟 crate。依赖图上它在最下游，因而看不见 storage / api /
  worker 的任何类型——`AppError::Storage` 用 `anyhow` 承接下游错误正是这个缘故。
- **不初始化日志**：只 `use tracing`，subscriber 安装全 workspace 仅在 `{{crate_prefix}}-app`
  的 `init_tracing`（测试里是 `{{crate_prefix}}-testkit` 的 `LogCapture`）。
- **无常驻任务、无 SQL**：`TaskSupervisor` 只登记与收割别人 spawn 的 `JoinHandle`，
  自己不 `tokio::spawn` 长活循环。
- **窄门面**：`lib.rs` 只做 `mod` 声明与显式 `pub use`，调用方走两级路径
  （`config::ConfigHandle`）。唯一扁平化的例外是错误基座：
  `use {{crate_prefix_snake}}_core::{AppError, AppResult}`。

## 目录

```text
src/
  lib.rs          窄门面 + 纪律声明；VERSION / SERVICE_NAME 两个进程级常量
  error.rs        AppError / AppResult；变体按失败面分，不按业务分
  events.rs       AppEvent / EventBus / EventStream：通知不载荷的丢弃式广播
  metrics.rs      Metrics：注入式自省计数，进程内无 static 计数器
  tls.rs          ensure_crypto_provider：进程级 ring provider 安装（Once 幂等）
  task/
    mod.rs        任务收敛三模式的说明（何处用何种）
    supervisor.rs TaskSupervisor：顶层任务登记、first-failure、限时收割、Drop 兜底
  config/
    mod.rs        窄门面导出
    app.rs        AppConfig / load / load_or_init / default_config_template
    default.toml  缺省配置模板（首次启动落盘的那份）
    store.rs      ConfigStore 写端 + ConfigHandle 读端 + 热重载与 COLD_PREFIXES 回滚
    env_expand.rs ${env.NAME:默认值} 占位符展开
    path_util.rs  相对路径按安装根解析（不是配置文件所在目录）
    clamp.rs      数值钳位的统一写法（越界不报错，钳位并 warn）
    error.rs      ConfigError：教学式错误（说清在哪、怎么修）
    cfg_*.rs      各段：http / runtime / storage / ticker
  util/
    build_info.rs BuildInfo：option_env! 注入的发布元数据 + 服务诊断串
    time.rs       TimestampMs = i64（UTC epoch 毫秒），now_ms
```

## 关键决策

**配置只有一条管线。** 启动与热重载共用 `load`：读文件 → 占位符展开 → 反序列化 →
路径解析 → 校验 → 钳位。两条管线迟早在某一步分家，于是「启动能起、reload 报错」
这类故障只在生产出现。

**写端与读端分家。** `ConfigStore` 唯一实例在装配层，`ConfigHandle` 是廉价 Clone 的读端。
面拿到的是读端，因而不可能顺手改配置。可热字段每 tick 现读，热重载下一拍生效。

**不可热的段名登记在一处。** `store.rs` 的 `COLD_PREFIXES` 列出 `runtime.` / `http.` /
`storage.` 这类需要重启的前缀；reload 时它们被回滚为运行值并在日志里列出
`requires_restart`。加不可热的段要同时改这张表，否则会出现「改了没生效也没提示」。

**事件通知不载荷。** `AppEvent` 只带键与代际号，数据本体由订阅者自己去共享态或库里取。
总线是丢弃式 broadcast，慢消费者 `Lagged` 丢事件不丢数据——订阅者的兜底轮询是真值，
事件只负责提前一拍。业务事件按同一原则往 `AppEvent` 加变体，不要另起第二条总线。

**指标是注入的，不是静态的。** `Metrics` 由 app 装配一份、以 `Arc` 注入写读两侧。
进程内没有 `static` 计数器——这是测试不加 `--test-threads=1` 就能并行、以及一个进程
里跑两套装配的前提。计数全用 `Relaxed`：这些数字之间不构成任何不变量。

**监管器全进程只有一个。** `JoinHandle` / `AbortHandle` 不绑定 runtime，任务 spawn 在
主 runtime 还是附加 runtime 对它没有区别；first-failure 是进程级语义，每 runtime 一个
监管器只会多一层聚合。`JoinSet` 里跑的是等待句柄的监控任务，所以还要单独留
`AbortHandle`——否则超时只中止了监控任务，底层任务脱管。

**时间基元不引 chrono。** 退避窗口与任务框架只需要一个可比较、可做差的整数时刻。
时长一律走单调钟（`Instant`），墙钟只用于展示与落库。

**rustls provider 显式装 `ring`。** rustls 0.23 必须指定 provider，而依赖表里走 rustls 的
crate 都不自带；`aws-lc-sys` 要 cmake 与 C 工具链，交叉编译时纯属负担。代价是首次用到
TLS 之前必须调 `ensure_crypto_provider`，漏了是运行期 panic 而不是编译错误——所以用
`Once` 做成幂等，任何入口调一次都不会漏装也不会重装。

## 测试形态

全部是同文件 `#[cfg(test)]` 单测，没有 `tests/` 目录：core 的公共面本身就是被测面，
黑盒测试留给有端点或有库的 crate。

| 模块               | 形态                                | 钉住什么                                                            |
| ------------------ | ----------------------------------- | ------------------------------------------------------------------- |
| `config/*`         | 同步，`toml::from_str` 直打反序列化 | 缺省值、`deny_unknown_fields`、钳位边界、占位符展开、错误文案       |
| `config/store`     | 同步 + 一条 `#[tokio::test]`        | 热重载的 applied / deferred / requires_restart 三分，失败保留旧配置 |
| `events`           | `#[tokio::test]`，容量给小值        | 订阅者收得到、`Lagged` 之后还能继续收                               |
| `metrics` / `util` | 同步                                | 快照字段、空白值兜底、时刻单调                                      |
| `task/supervisor`  | `#[tokio::test]` 真实时钟           | first-failure、超时 abort、Drop 兜底不脱管                          |

配置类测试要落盘时用 `std::env::temp_dir()` 拼 pid + 纳秒的目录，不引 testkit：
core 是叶子，dev-dependencies 也不例外（testkit 依赖 core，反过来就成环）。
