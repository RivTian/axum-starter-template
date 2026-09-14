# 架构设计：多 crate 分层 + 受监督任务面

- **版本**：v0.8，生成物与维护历史解耦；当前回归状态见 `docs/verification.md`，阶段记录保留为历史证据。
- **日期**：2026-09-14。
- **决策状态**：用户于 2026-09-14 接受 §14.3 的全部四项 P0；后续按本文实施，不再作为待选方案重复确认。
- **范围**：本文定义完整模板的目标契约，实际实现进度见 §13。M5 已补齐名称/依赖矩阵、生成项目门禁、真实迁移失败与协议边界探针、文档和 CI 定义。55 项证据映射见 `docs/acceptance.md`；本地运行与远端/其他平台的验收状态分别记录，不把 workflow 文件存在当成平台通过。
- **基线**：继承 `edge_dev` 的分层思想与可验证的工程纪律，不继承业务模型；备份分支只作取证和工程件来源，不作实现基线。
- **阅读方式**：先读 §1 的取舍、§2 的纪律，再审 §4–§9 的失败路径；§12 是实现验收清单，§14 记录已确认决策、技术风险和实施中待验证项。

## 目录

1. [背景、目标与现状取证](#section-1)
2. [纪律清单](#section-2)
3. [crate 分层、目录与所有权](#section-3)
4. [runtime 拓扑与可选绑定](#section-4)
5. [任务监督与进程生命周期](#section-5)
6. [横切能力准入清单](#section-6)
7. [三段式配置与热重载](#section-7)
8. [单后端存储、迁移与 fail-fast](#section-8)
9. [HTTP 契约与就绪语义](#section-9)
10. [错误处理与可观测性](#section-10)
11. [模板生成与工程链复用](#section-11)
12. [测试策略与验收矩阵](#section-12)
13. [分阶段实施计划](#section-13)
14. [方案取舍、风险与决策记录](#section-14)
15. [证据索引与官方语义依据](#section-15)

---

<a id="section-1"></a>

## 1. 背景、目标与现状取证

### 1.1 问题定义

这不是在旧模板上补几项功能，而是重新定义一份可以复制、删减、投入项目的服务起点。
旧方案曾经令人满意，但“代码里有 supervisor / token / timeout”不等于生命周期闭环：
任务究竟由谁持有、超时后还会不会无限等、配置声称生效的值是否真的被消费，都需要明确证据。

本次设计以**当前消费者、显式所有权、可观测失败、自动化验收**为准入条件。
不为了保留旧目录而保留抽象，也不把源项目的现场约束提升为通用规则。

### 1.2 目标

生成的服务是一份能起、能停、能测的 Rust Web 服务起点，包含：

- workspace 多 crate 单向分层；
- 默认一个多线程 Tokio runtime，由 `TaskSupervisor` 持有顶层任务；
- 根 `CancellationToken` 向子任务级联，协作取消、限时收割、限时 abort 后收割；
- `Arc<dyn Storage>` 门面、sqlx **单后端**、迁移和启动 fail-fast；
- 启动与重载共用的三段式配置管线，以及 `watch` 最新配置分发；
- tracing 只在装配层初始化；所有服务状态按实例构造和注入；
- 可选的任务面 runtime 绑定：独立执行线程预算，不配置时不创建额外 runtime、线程、转发任务或队列。

“限时”指协调器和操作系统仍能调度时的应用级有界等待，不是强杀任意线程的能力。
“零代价”指可选拓扑的**未启用运行开销为零**，不是声称没有相关源码或配置校验代码。详见 §4.4、§5.7。

### 1.3 非目标

- **不是框架**：没有 `trait Service`、插件注册、自动发现、通用任务工厂、自研宏或 proc-macro crate。
  普通函数和具体类型足够；这里不禁止依赖提供的 `serde` derive、`tokio::select!`、测试宏和 `sqlx::migrate!`。
- **不带业务**：仅一个示例常驻任务面 `ticker`，以及 `health`、`ready`、`info` 三个系统端点。
- **不预铺**：不建业务表，不放无人使用的 `AppState` 字段，不建空 `domain`、`repo`、`middleware` 目录。
  唯一无生产 handler 消费者的预置代码是 `api/src/extract.rs`，判据见 §6。
- **不带部署事实**：不继承交叉编译目标、glibc 门禁、容器链、固定服务端口、安装根目录或特定编排平台。
- 不带 `reconcile` 期望集引擎、任务重启策略、事件总线、指标后端、鉴权、CORS、TLS 终止、Web UI、OpenAPI、业务配置写 API。
- 不承诺从任意旧服务数据库原地升级；也不把进程关停失败等同于数据库事务已回滚。

### 1.4 已检查的事实

以下保留 M0 初始取证时的历史快照；M1 新增实现与证据另见 §13 和 `docs/m1-verification.md`。
未获取同事的完整审计报告，以下不是对该报告的复述，也不是完整安全审计。

| 来源 | 已核验事实 | 本次使用方式 |
| --- | --- | --- |
| 当前模板仓库 | `main` 位于 `71b5540`，只有初始化提交；工作树已有未跟踪的 `.gitignore`，`docs` 原为空目录 | 从空骨架设计；不覆盖该 `.gitignore`，不恢复旧业务代码 |
| `origin/backup/main-20260913-multi-rt-design` | 本地远端跟踪引用指向 `1ac758474aa09c0e5f31ef93547db5a9e7c5fcbb` | 按这个对象取证，不假定它代表远端此刻的最新状态 |
| `edge_dev` | `/Users/riotian/Documents/code/quasar/prism/edge_dev`，HEAD 为 `8ef215323facace7f6fb5caa6ddfc662c15f508a`，取证时工作树干净 | 学习多 crate 边界、存储门面、装配顺序、测试方法；不复制设备、告警、采集或推送模型 |

具体观察及其边界：

| 观察位置（完整路径见 §15.1） | 实际观察 | 新设计决定 |
| --- | --- | --- |
| `edge_dev` 的根 manifest 与 `app/src/boot.rs` | 已有 api/app/core/provider/push/runtime/storage/testkit 分层；装配层连接存储并注入能力 | 保留单向依赖、装配注入；删掉业务面和无消费者共享件 |
| 两份源码的 `core/src/task/supervisor.rs` | `JoinSet` 持有等待已有 `JoinHandle<()>` 的 watcher，另存 `AbortHandle`；abort 后循环收割没有第二个 deadline | 监督器移入唯一消费者 app；直接持有实际 future；两段收割都有期限 |
| 备份的监督器 | 重名只用 `debug_assert!`；任务结果只表达 `()` 和 `JoinError` | release 也拒绝重名；保留业务 `Err`、panic、取消和意外 `Ok` 四种退出 |
| `edge_dev` 与备份的装配 | HTTP 最后注册作为就绪纪律 | 顺序不构成就绪证明；增加目标 runtime 上的启动确认与启动提交门 |
| 备份的 `app/src/rt.rs` | 每个 runtime 获得完整的相同关闭宽限；源码在调用 `shutdown_timeout` 后记录 stopped | 改为所有 runtime 共用剩余预算；仅记录该调用返回，不冒充线程全停 |
| 备份的 `core/src/config/store.rs` | 冷字段分类与回滚两处维护；未分类字段落入 applied，另有 enabled 的 deferred 语义 | 热/冷类型分离；只允许明确热字段；混合冷热变更整批拒绝，不做“半热” |
| 备份的根 manifest | release 使用 `panic = "abort"` | 不继承；监督模型要求支持展开的目标上使用 unwind，并用真正的 release 二进制验证 |
| 备份的 API | 提取器包装有价值；但根部未知路由、405、超时响应仍须逐条核验 | 保留统一错误契约的意图，补全契约测试，不声称所有网络错误都能变 JSON |
| 备份的生成器、Makefile、CI、同步脚本 | 已区分模板仓库与生成项目，已有名称格式矩阵和 clean-room 生成验证 | 复用工作流思想；删除双后端任务并重新审查替换、清理和缓存边界 |

### 1.5 本版关键取舍

1. 初始 workspace **五个 crate**：`app`、`api`、`worker`、`storage`、`core`。
2. `TaskSupervisor`、runtime 集合、文件配置写端、OS 信号属于 **app**，不放进“什么都装”的 core。
3. 单后端确定为 **SQLite**：用户已接受此基线，新生成项目无需外部数据库进程即可验证生命周期。
   本版不同时提供 PostgreSQL 代码或后端选择器；将来替换后端属于新的明确需求，不是本次实施的待选项。
4. ticker 恒驻，只有周期可热改；没有 `enabled` 的注册/运行双重含义。删除示例是删代码，不是加一个停摆开关。
5. 热重载是**配置发布**，不是任务重建；runtime、绑定、监听、池参数、关闭预算均重启生效。
6. HTTP 使用 axum 的简洁服务入口，不为骨架重写 Hyper 连接管理器；其强制终止边界必须如实披露并测试。
7. 不复制旧文档的外部案例结论；影响正确性的 Tokio、sqlx、axum、Rust 语义按官方资料重新核对，见 §15.2。

---

<a id="section-2"></a>

## 2. 纪律清单

每条纪律都必须在实现旁有“为什么”，并在 §12 留下自动化证据。以下路径均指 §3 的拟建文件，不是当前已有实现。

### A. 依赖与抽象

| 编号 | 纪律 | 理由 / 落点 |
| --- | --- | --- |
| A1 | 只允许 §3 邻接表中的生产依赖；core 无兄弟依赖 | 从 manifest 层阻止逆向渗透；用 `cargo metadata` 验边 |
| A2 | 第三方版本统一管理，成员只开实际所需 feature；没有消费者就不引入依赖 | 不使用 Tokio `full`、sqlx 全后端等省事开关 |
| A3 | 抽象因替换边界或真实复用而存在，不为目录对称存在 | `Storage` 有 API fake 的消费者；通用 Service / Reconcile / EventBus 没有 |
| A4 | 门面不暴露池、连接、事务或 sqlx 错误类型 | 存储替换不把签名变更传播给 API |
| A5 | 常驻面导出普通入口函数；不自动 spawn，不创建 runtime | 任务所有权和执行位置由 app 唯一决定 |
| A6 | core 不读文件、不访问环境、不注册信号、不访问数据库、不持有常驻任务 | 共享“契约”不等于共享“装配” |
| A7 | 不设通吃全仓库的 `AppError`；各层错误在边界显式转换 | 不靠遍历 `source()` 猜 HTTP 语义 |

### B. 监督与关停

| 编号 | 纪律 | 理由 / 落点 |
| --- | --- | --- |
| B1 | app 自有的顶层常驻 future 仅经监督器直接 spawn 并取得所有权；不接受已 spawn 的 `JoinHandle` | 直接所有权消除 watcher 脱管窗口；`app/src/supervisor.rs` |
| B2 | 服务 Running 时，任一顶层任务返回 `Ok`、`Err`、panic 或被意外 abort 均触发进程级关停 | 模板不容忍“HTTP 活着但工作面死了”；不自动重启 |
| B3 | 根 token 仅 app 能取消；面拿 `child_token()`，不能取消兄弟 | 表达父子取消权限；子任务也必须有持有者 |
| B4 | 先撤销 readiness，再 cancel，再限时收割；abort 后仍必须限时收割 | `abort()` 是请求，不是完成证明；§5.6 |
| B5 | 每个存活的自有子任务都有 join 所有者；Drop 只兜底请求 abort | 禁止丢 JoinHandle 当清理；第三方内部任务另列边界 |
| B6 | 启动失败与正常停止共用清理路径；保留首因及清理结果 | 不以 close 超时覆盖真正的 bind / migrate 错误 |
| B7 | 每个阶段使用绝对 deadline，runtime 数量不会乘大总预算 | 无界 await、逐任务完整 timeout 都不合格 |
| B8 | panic 策略与承诺一致；不采用 release `panic=abort` | 否则根本没有机会执行监督和关停；§10.2 |

### C. 配置

| 编号 | 纪律 | 理由 / 落点 |
| --- | --- | --- |
| C1 | 启动、重载共用同一解析/补全/校验管线；最终配置类型有明确冷热边界 | 避免热加载绕过校验 |
| C2 | 唯一写端由协调器串行持有；读端只有快照与变更等待能力 | `&mut` 所有权比多写者加锁更简单 |
| C3 | 未知字段、非法范围、缺失环境变量明确失败；不静默钳位 | 不继承源项目“无人值守比正确值更重要”的现场假设 |
| C4 | 冷字段改变时整批不发布；相同配置不增代、不通知 | 不让磁盘值、运行值和报告产生第三种“部分生效”解释 |
| C5 | generation 和热值放在同一个不可变快照中，一次替换 | 不读到“新代号 + 旧值” |
| C6 | 不跨 await 持有 watch borrow / 同步锁；不把 watch 当事件日志 | 慢消费者可以跳代，但必须收敛到最新配置 |

### D. 存储与状态

| 编号 | 纪律 | 理由 / 落点 |
| --- | --- | --- |
| D1 | 一种数据库、一套驱动 feature、一个迁移目录、一个池 | “单后端”不是运行时只选一个、编译时仍带两个 |
| D2 | 连接、迁移、自检全部成功前不提交启动 | lazy pool 被构造不等于数据库可用 |
| D3 | 不建任何业务表；迁移元数据表不属于业务预铺 | 第一条业务迁移必须随真实用例加入 |
| D4 | 正常路径先排空使用者再关池；强制路径不得假报排空完成 | 池关闭也有独立预算；§5.6、§8 |
| D5 | 业务及运行状态按实例注入；不使用可变业务 static / OnceLock 单例 | 一个进程内能并行装配两套服务 |

### E. HTTP

| 编号 | 纪律 | 理由 / 落点 |
| --- | --- | --- |
| E1 | 只有 `/v1/service/{health,ready,info}` 三个系统端点，无另设探针别名 | 一套路由契约即可；未知路径另有统一 fallback |
| E2 | health 不访问数据库；ready 检查生命周期与有界存储探测 | 区分存活、已装配和依赖可用性 |
| E3 | 层逻辑直接放 router 装配处；不预建 middleware 目录 | 当前层数少，不提前抽象 |
| E4 | 应用可生成的 404、405、超时、提取失败和内部错误统一映射 | 不把 handler 之外的失败漏出契约 |
| E5 | `AppState` 每个字段都有当前 handler / layer 消费者 | 没人读取配置就不注入整个 ConfigHandle；没人读指标就不放 Metrics |
| E6 | 提取器包装保留框架状态码；5xx 文本泛化；错误不回显密钥和请求体 | `api/src/extract.rs` 是唯一预置例外 |
| E7 | API 不持有配置写端、根 token、runtime 或 supervisor | HTTP 不成为第二个装配中心 |

### F. 测试、文档与工程件

| 编号 | 纪律 | 理由 / 落点 |
| --- | --- | --- |
| F1 | 单元测状态机；router 测响应；进程测信号、真实 runtime 与 release | 测试层级与故障边界一致 |
| F2 | 用握手、屏障和虚拟时间，不用固定 sleep 猜就绪 | 超时只当失败上限，不作同步工具 |
| F3 | 不绑定固定测试端口，不改进程全局环境，不装全局测试 subscriber | 支持默认并行测试 |
| F4 | 模板维护门禁和生成项目门禁分开；生成后执行 `--locked` 门禁 | 占位符源码不能冒充真实生成结果 |
| F5 | 工程件按文件复用、按新契约验收，不整体恢复备份 | 它们可复用不代表其中每个参数都通用 |
| F6 | 不用“冻结语义”免除再评审；改纪律必须同时改文档、代码与测试 | 旧设计已经证明注释本身不是正确性证明 |

---

<a id="section-3"></a>

## 3. crate 分层、目录与所有权

### 3.1 生产依赖图与邻接表

箭头为“左侧依赖右侧”，不是启动顺序：

```text
app ──→ api ──────→ storage
 │       └───────→ core
 ├────→ worker ──→ core
 ├────→ storage
 └────→ core

storage：无兄弟依赖
core：无兄弟依赖
```

| crate | 允许的直接生产依赖（workspace 内） | 对外边界 |
| --- | --- | --- |
| `svc-app` | api、worker、storage、core | 一个二进制；负责装配和退出码，不供其他 crate 依赖 |
| `svc-api` | core、storage | `HttpSettings`、`AppState`、router 构建、HTTP 服务 future |
| `svc-worker` | core | ticker future 与局部错误；不持有 Storage，因为示例不落库 |
| `svc-storage` | 无 | `StorageConfig`、`StorageOwner`、只读 `Storage` 门面、不透明错误 |
| `svc-core` | 无 | 热配置快照/读写能力、生命周期快照/读写能力、构建信息等已被共享的类型 |

`storage → core` 不是为了“像一层”而强加的边：本版存储没有使用 core 的真实需求。
以后产生共享领域值对象时再评审添加；不能先引入再保留一个空 `use`。

`svc` 只是文档示例前缀。生成器使用 `crate_prefix` 形成真实包名；M1 采用固定的内部依赖别名
`service_core` / `service_api` / `service_storage` / `service_worker`，不把包名前缀插入 Rust 源码。
项目名和二进制名仍可以不同于包名前缀；这个工程调整消除了名称长度引起的源码排版变化，详见 §11。

M2 已按真实消费者落实上述生产依赖边：app 依赖 api/worker/storage/core，api 依赖 core/storage，worker 依赖 core，storage/core 不依赖兄弟 crate。
core 当前实现 BuildInfo、TickerInterval、生命周期读写能力，以及 M3 的 HotConfig/ConfigSnapshot/ConfigPublisher/ConfigHandle；加载与提交策略仍只属于 app。

### 3.2 拟建目录

下树是实施目标，不是要求一次建完所有空文件；各文件在其消费者落地时一起提交。
根目录为 `/Users/riotian/Documents/code/axum-starter-template`，生成项目保留相同相对布局。

```text
axum-starter-template/
├── Cargo.toml / Cargo.lock / rust-toolchain.toml
├── cargo-generate.toml / .gitattributes
├── Makefile / Makefile.project
├── README.md / README.project.md
├── project-ci/workflows/ci.yml          # 生成项目 CI 的源，post hook 移入 .github
├── config/service.toml                # 生成项目的本地演示配置，不是部署规范
├── app/
│   ├── Cargo.toml / README.md
│   ├── src/
│   │   ├── main.rs                    # 同步入口，持有 RuntimeSet
│   │   ├── cli.rs                     # 命令行参数解析
│   │   ├── config/                    # 解析、校验、重载分别有真实职责
│   │   │   ├── mod.rs                 # RawConfig / BootConfig / Candidate
│   │   │   ├── load.rs                # 默认路径、来源选择与三段式加载
│   │   │   ├── runtime.rs             # 冷拓扑、名字/引用/预算校验
│   │   │   ├── reload.rs              # 单飞加载、冷热比较、唯一提交点
│   │   │   └── {tests.rs,reload/tests.rs} # 拆出的单元测试模块
│   │   ├── boot.rs                    # 启动资源所有者与启动提交
│   │   ├── supervisor.rs              # app 私有，直接持有实际任务
│   │   ├── shutdown.rs                # deadline、收割报告、统一清理
│   │   ├── signals.rs                 # OS 适配，不承载状态机
│   │   ├── rt.rs                      # 可选 runtime 拓扑及同步销毁
│   │   ├── telemetry.rs               # 唯一 tracing 初始化
│   │   └── {boot,rt,supervisor}/tests.rs # 同上；shutdown/cli/telemetry 的测试内联在各文件末尾
│   └── tests/                         # cargo 集成测试与进程级驱动并存
│       ├── runtime_semantics.rs       # 进程内多 runtime / JoinSet 所有权语义探针
│       ├── check_service.py           # 真进程：端点契约、信号、重载、fail-fast、拓扑矩阵
│       ├── check_logging.py           # 真 stdout：PTY / 管道 / 文件三种 sink
│       ├── check_release.py           # 真 release 二进制的受控 panic 与退出码
│       └── fixtures/release_panic.rs  # 独立 release 回归夹具，不是服务使用示例
├── core/
│   ├── Cargo.toml / README.md
│   └── src/{lib.rs,config.rs,lifecycle.rs} # 单元测试内联在各文件末尾
├── storage/
│   ├── Cargo.toml / README.md / build.rs
│   ├── migrations/README.md           # 起始无业务 .sql 文件
│   └── src/{lib.rs,tests.rs,migration_tests.rs} # 真 SQLite 与测试专属迁移 fixture，用私有 pool
├── worker/
│   ├── Cargo.toml / README.md
│   └── src/lib.rs                     # 单元测试内联在文件末尾
├── api/
│   ├── Cargo.toml / README.md
│   ├── src/{lib.rs,state.rs,error.rs,response.rs,extract.rs}
│   ├── src/handler/{mod.rs,system.rs}
│   └── src/{contract_tests.rs,tests.rs} # Router 契约与真实连接测试，用私有 extract / finish
├── hooks/{pre.rhai,post.rhai}
├── scripts/                           # 仅模板维护的生成/格式/结构验收脚本
├── .github/workflows/ci.yml            # 模板自己的 CI
└── docs/{architecture.md,acceptance.md,verification.md} # 仅供模板维护
```

不建 `reconcile`，也不预建 `testkit`。共享测试 crate 只有在两个以上 crate 确实重复同一资源夹具、
且不会引入生产反向依赖时才允许增加；本版优先 crate 本地测试模块和少量显式 fake。

**测试落点由可见性决定，不是风格选择。** app 是纯二进制 crate（只有 `[[bin]]`，无 `[lib]`），
`tests/` 无法导入它的模块，因此 boot / config / rt / supervisor / shutdown / cli / telemetry 的用例
只能是 `src/` 内的 `#[cfg(test)] mod`；为了让它们搬进 `tests/` 而给 app 加 `[lib]` 目标，
等于把装配层内部导出成库公共面，与 §3.3"`TaskSupervisor` 不向 api/worker 导出"直接冲突。
api 的契约测试使用私有 `extract` 包装器与私有 `finish`，storage 的测试使用私有连接池与测试专属
`Migrator`，同样不能搬进各自的 `tests/`。只有确实仅依赖公共面的用例才放 `tests/`；
把大体量测试模块拆成独立文件只是体量决策，不改变它们仍是单元测试这一事实。
`app/tests/` 因此同时存放三类 cargo 语义不同的东西：集成测试目标、`[[example]]` 回归夹具、
以及 cargo 完全不运行的进程级驱动（见 §12.1）。

### 3.3 所有权账本

| 对象 | 唯一写入/销毁所有者 | 被注入的能力 | 禁止 |
| --- | --- | --- | --- |
| `RuntimeSet` | 同步 `main` | app 注册任务时短暂借用 `Handle` | 把 `Runtime` 塞入 AppState 或让 async Drop 销毁 |
| `TaskSupervisor` | app 协调器 | 不向 api/worker 导出 | 第二个 supervisor 聚合层、隐式重启 |
| 根 token | app 协调器，异常退出 guard 兜底 | 每个面的 child token | 给面 root 的 clone，扩大取消权限 |
| 最终截止时间上下文 | main 持有，借给 app 写入一次 | 同步 teardown 沿用相同 deadline | panic 或阶段切换时重新计时 |
| 配置文件路径、BootConfig | app | 只把已验证的必要参数传给消费者 | core 自己定位文件或读取环境 |
| 热配置 publisher | app 协调器 | ticker 只拿 read handle | API 修改文件或发布配置 |
| 生命周期 publisher | app 协调器 | API 拿只读状态 | handler 自行宣布 Running |
| 池及启动中间态 | storage 启动会话，完成后由 app 管理关闭 | API 得 `Arc<dyn Storage>` | api 取得连接/迁移能力 |
| BuildInfo | app 构造 | info handler 的不可变字段 | 运行时执行 git、读取任意环境或扫描部署路径 |

publisher 不实现 `Clone`，不公开底层 sender；用普通构造函数返回一对能力。
即便 core 定义 publisher 类型，也不意味着 core 启动后台任务或拥有业务状态单例。

### 3.4 依赖与 profile 策略

- workspace 统一 edition、MSRV、版本、lint 与依赖版本；M1 已固定 Rust/MSRV `1.97.1`、Tokio `1.53.1`、Tokio-util `0.7.19`、axum `0.8.9`、sqlx `0.9.0`、cargo-generate `0.24.0` 并验证生成结果，完整清单见 M1 报告。该 MSRV 是验证基线，不宣称最低可能版本；不把“latest”作为构建输入。
- Tokio 根据使用点启用 runtime、多线程、sync、time、net、signal、macros；`test-util` 只用于测试。
- sqlx 只开 SQLite、Tokio 集成和迁移所需 feature；不引入 PostgreSQL、MySQL、Any、查询宏离线缓存或 TLS provider。
  M1 使用 `sqlite-bundled`，不使用还包含扩展加载等能力的 `sqlite` 聚合 feature；迁移所需的 `macros` 会携带 derive/offline 支持，但本模板不使用 query! 或生成离线查询缓存。
  最终 feature 闭包必须核查实际选中的 workspace 构建/测试树；不能只看直接 manifest，也不能将 metadata/lock 中未启用的可选依赖误判为已编译后端。
- `tracing-subscriber` 只属于 app 的生产依赖；其他 crate 使用 `tracing`，不初始化 subscriber。
- 测试可引入 `tempfile`、`tower::ServiceExt` 等真实需要的工具；不把测试便利类型推到生产公共面。
- 默认保持 release `panic = "unwind"`；不自动继承极限体积 profile、strip 组合或目标特定链接参数。
  LTO 等优化以后按测量取舍，不能牺牲本设计的失败可观测性。[R8]

---

<a id="section-4"></a>

## 4. runtime 拓扑与可选绑定

### 4.1 默认路径

**M4 实现落点**：`app/src/rt.rs` 的 RuntimeSet 由同步 main 独占；Executors 是仅供装配层启动解析的 Handle 集合，不传入 api/worker。
`TaskSupervisor` 的退出记录带实际 runtime 名称。默认 extra Vec/Handle Vec 均无分配，main 标签借用静态字符串；只构建一个 runtime，不增加 watcher 或转发队列。

```text
同步 main：CLI → 配置加载/校验 → tracing → 构造主 runtime
                                    │
                         main_runtime.block_on(run)
                                    │
                      协调器 + 一个 TaskSupervisor
                          ├── ticker future
                          └── http future
                                    │
                        显式清理 → block_on 返回
                                    │
同步 main：关闭额外 runtime（若有）→ 关闭主 runtime → 返回 ExitCode
```

使用同步 `fn main` 而不是 `#[tokio::main]`，是因为 runtime 参数必须先从配置读取，
并且所有 runtime 都必须在异步上下文之外明确销毁。构建信息和配置错误在日志尚未初始化时走 stderr。

所有 runtime 使用 `Builder::new_multi_thread()`；主 runtime 的 worker 数缺省时由 app 读取
`available_parallelism()` 并解析为正数，再显式传给 builder，解析失败保守取 1。
这个推导值固定在启动时的 LoadContext 中，重载不重新探测 CPU 数。
不让 `TOKIO_WORKER_THREADS` 成为隐蔽的第二配置入口；阻塞池上限也由配置解析得到。

单 worker 的隔离方式是“多线程调度器 + `worker_threads = 1`”，不提供 current-thread 选项。
额外 runtime 没有专门的外层 `block_on` 驱动者，不能误把只有 Handle 的 current-thread runtime 当作能自行运行。

### 4.2 显式绑定

**当前边界值（M4）**：extra 名称为最多 32 字节的小写 kebab-case，`main` 保留；两个线程预算都必须显式提供。
每个预算为正，最多 8 个 extra，worker 总数和 blocking 上限总和分别不超过 256；当前只有两个任务面，其他未被引用的定义仍会被拒绝。
完整校验既在文件加载后执行，也在 RuntimeSet 构建入口执行，防止程序式调用绕过；不在失败时回落到主 runtime。

以下是**可选片段**，不是生成结果的默认配置：

```toml
[runtime.extra.compute]
worker_threads = 1
max_blocking_threads = 2

[ticker]
interval_ms = 1000
runtime = "compute"
```

- 不写 `<面>.runtime` 或写 `"main"`：绑定主 runtime。
- 写其他名字：必须存在同名 `runtime.extra.<name>`；不认识的名字是启动错误，不回落到主 runtime。
- `main` 是保留名；extra 名称须满足文档化的有限字符/长度约束。
- 未被任何面引用的 extra 定义报错，避免建出空闲线程池还以为已有隔离。
- `http.runtime` 和 `ticker.runtime` 都是冷字段；运行中不迁移任务、不重建 runtime。
- runtime 配置、所有绑定和总预算先全量校验，再创建任何 runtime。
- 多个 extra 用确定顺序构建；第 k 个构建失败，已有 runtime 也必须经显式有界关闭，而不是落入隐式无限 Drop。

不引入通用 Executor trait，也不把 handle map 注入 worker 或 API。
app 在每次启动注册时解析目标 handle，监督器调用 `JoinSet::spawn_on` 直接把实际 future 放入该 runtime。[R1]

### 4.3 跨 runtime 资源规则

| 资源 | 创建/使用规则 | 关闭规则 |
| --- | --- | --- |
| `Arc`、Tokio sync 通道、CancellationToken | 在 `Send + Sync` 契约内跨 runtime 传递；不带执行器代理 | 发布者和 token 所有权仍在 app |
| 面私有 timer / I/O | 在目标 runtime **开始轮询后的 async 函数体内**创建 | 面正常返回前完成自己的资源清理 |
| HTTP listener | 在 http future 的目标 runtime 内异步 bind；绑定成功后回报实际地址 | 等待启动提交后才 accept；graceful 结束才确认连接排空 |
| sqlx 池 | 在主 runtime 执行启动流程；不由面各开一份 | 正常路径在所有使用者排空后 close，主 runtime 最后关闭 |
| 自有阻塞工作 | 进入目标 runtime 的 blocking 池前就要有限并发、有限排队 | 持有 join；需要库级超时/协作取消，不能只依赖 abort |

跨 runtime 不等于移动 I/O 注册。不同后端/库的内部线程实现也不相同：SQLite 驱动可能有自己的工作线程，
不能把它们都算作 Tokio worker。保守保持“共享资源先排空，再关池，再关主 runtime”的所有权顺序。[R2][R4]

禁止在 runtime worker 上嵌套 `block_on`，不使用 `block_in_place` 伪装 CPU 隔离，
不在构造 future 的同步前缀提前创建目标 runtime 资源。

### 4.4 线程预算与“零代价”的准确含义

**M4 本地证据**：五种布局（main、仅 ticker extra、仅 HTTP extra、共享 extra、独立两个 extra）都运行了普通 debug/release 服务进程。
测试校验任务实际线程上下文、共享存储 readiness、冷拓扑重载拒绝、关池收据先于 runtime teardown 以及 extra 逆序/main 最后的日志顺序。
默认路径只构建一个 runtime 的证据与“额外 worker 被阻塞时主 HTTP 仍响应”的隔离证据分开，不将后者推广成 CPU 配额或任意阻塞代码的硬截止保证。

运行时启动日志分别记录：

- 每个 runtime 的 worker 数、blocking 池上限、绑定面；
- worker 数总和与 blocking 上限总和；
- 明确这些是执行资源配置，**不是进程总线程数**，也不是 CPU 核绑定或 CPU 时间配额。

没有 extra 配置时：

1. 只构建一个 runtime；extra 集合为空，不为它创建线程或队列。
2. ticker / HTTP 都直接 spawn 到主 runtime；无每任务 watcher、跨 runtime 转发 future 或中转 channel。
3. HTTP 请求和 ticker 每一拍均不查 runtime 名称，不走动态 executor 分派。
4. 同一份监督/关停算法处理两种拓扑，不维护第二套业务路径。

仍然存在少量启动期配置解析代码和空集合字段，这是**源码复杂度**，不能宣传成逐字节零成本。
使用多 runtime 隔离的是调度队列及 worker / blocking 预算，不是内存、进程故障或机器 CPU 的硬隔离。
长 CPU 运算应切块让出或放入有界计算池；不能终止的 FFI / 阻塞调用，若要求硬截止，应使用可终止的子进程，
而不是继续叠加 Tokio runtime。[R2][R3]

---

<a id="section-5"></a>

## 5. 任务监督与进程生命周期

### 5.1 概念与状态机

**任务面**是进程希望在 Running 期间持续存在的工作，不是一个 trait。
本版只有 ticker 和 HTTP；信号等待是协调器的一个分支，重载文件是协调器持有的有限作业，
都不伪装成额外业务面。

```text
Booting ──全部前置条件 + 启动确认──→ Running
   │                                  │
   └────启动错误 / 停止请求────────────┤
                                      ↓
                                   Draining
                                      │
                             宽限耗尽 / 第二次停止请求
                                      ↓
                                    Forcing
                                      │
                    有界收割 + 资源清理 + 同步 runtime 销毁
                                      ↓
                                    Stopped
```

- `Running` 是 app 唯一提交的状态，面只能报告“自己的初始化完成”，不能直接发布就绪。
- `Stopped` 只出现在最终退出报告中，不在 runtime 销毁后继续发布 watch 通知；它不等于所有任务都是 graceful。
- 进入停止路径后禁止注册新任务、提交配置和重启任务；第一原因只写一次，后续失败追加为清理结果。
- 状态快照通过 core 的只读 lifecycle handle 供 API 和启动门消费者读取；不建 broadcast 事件总线。

### 5.2 监督器接口草案

**M2 落点**：实际实现位于 `app/src/supervisor.rs`，由 app 独占；TaskError 是装配层私有的 boxed error 载体，面仍返回具体错误类型，API 不依赖或向下拆解这个载体。
监督器保持任务名和四类退出；`app/src/boot.rs` 执行绝对 deadline 收割，`shutdown.rs` 保存首因及退出预算；不是为所有 crate 引入通用 AppError。

下列代码是边界草案，省略私有类型和错误定义，不是可直接复制的完整实现：

```rust
// app 私有；TaskError 在 app 中按实际面错误显式适配。
type TaskResult = Result<(), TaskError>;

struct TaskSupervisor {
    tasks: tokio::task::JoinSet<TaskResult>,
    meta: std::collections::HashMap<tokio::task::Id, TaskMeta>,
}

impl TaskSupervisor {
    fn spawn_on<F>(
        &mut self,
        name: &'static str,
        runtime_name: &str,
        handle: &tokio::runtime::Handle,
        future: F,
    ) -> Result<(), RegisterError>
    where
        F: Future<Output = TaskResult> + Send + 'static;

    async fn next_exit(&mut self) -> Option<TaskExit>;
    fn abort_remaining(&mut self);
    async fn harvest_until(&mut self, deadline: tokio::time::Instant) -> HarvestReport;
}
```

关键行为：

1. `spawn_on` 在 release 也检查重复名字和停止状态；出错时不得先 spawn 再返回错误。
2. future 参数在移交前不执行副作用；实际创建 I/O/timer 的代码在 async 函数体内。
3. `JoinSet` 直接持有 `Future<Output = TaskResult>`；登记使用返回的 AbortHandle 的 task id 建立元数据，
   随后不必额外保存一组底层 AbortHandle。整个登记过程不 await，因此协调器不可能在元数据插入前收割它。
4. `join_next_with_id` / `JoinError::id()` 将返回结果和 panic 对应回任务名、目标 runtime；收割同时删除元数据。
5. `Result<Result<(), TaskError>, JoinError>` 不压扁：领域失败和运行时失败分别记录。
6. Drop 由 JoinSet 兜底请求 abort；Drop 不能 await，因此不作为成功关停路径。[R1]

**不使用旧的 watcher-of-JoinHandle 模型**；不以 `JoinSet::shutdown()` 代替本设计的有界收割，
因为它不能表达本设计的 deadline、逐任务原因和未收割名单。

### 5.3 退出分类与 first-failure

| 观察时状态 | 任务结果 | 处理 |
| --- | --- | --- |
| Booting / Running | `Ok(())` | 常驻面意外结束，记录 `UnexpectedReturn`，关停，非零退出 |
| 任意状态 | `Err(TaskError)` 或 panic | 保留名称、runtime、错误阶段；关停或追加清理失败，非零退出 |
| Booting / Running | 未由本次清理发起的取消 | `UnexpectedAbort`，关停，非零退出 |
| Draining | `Ok(())` | 正常协作退出；HTTP 另满足 §9.5 的排空证明 |
| Forcing | 本次 abort 后的取消 | 标记 `Aborted`，不是 `Graceful`；非零退出 |
| 任意停止阶段 | 到期仍未 join | `Unreaped`，保留任务名单，进入 runtime/进程退出路径 |

信号与任务退出同时已可读时，协调器先处理任务退出，避免把已观察到的失败洗成正常 Ctrl-C。
关停后观察到的 panic / `Err` 仍升级失败；`Ok` 以协调器观察到的生命周期阶段分类，不声称能还原跨线程纳秒级先后。
任务自身的循环则优先检查取消：这与协调器的 first-failure 优先级是两个不同位置的规则。

空集合的 `next_exit` 返回 `None`，不能在 `select!` 中形成忙循环。
监督器单测允许空集立即完成；产品启动路径若没有 HTTP 面，则是装配错误，不是一个健康的空服务。

### 5.4 启动协议：准备不等于提交

启动分为**可失败准备期**和**单点提交**；不是分布式事务，也不会回滚已执行的迁移。

| 顺序 | 动作与所有者 | 失败/取消处理 |
| --- | --- | --- |
| 1 | 同步 main 解析 CLI，选定并绝对化配置路径，运行三段式配置管线 | stderr 报配置错误；尚无任务、池或 runtime |
| 2 | app 初始化 tracing，记录 BuildInfo 与脱敏的配置来源 | 初始化失败明确退出；库和测试不争用全局 subscriber |
| 3 | main 构造所有 runtime；运行 app 协调器 | 部分构造失败也显式关闭已经建好的 runtime |
| 4 | 先注册长期持有的 OS 信号接收器；建立根 token、Booting 状态和启动 deadline | 注册失败进入统一清理；不能等到 HTTP 起好才开始接收停止信号 |
| 5 | app 持有 `StorageOwner`，执行连接 → 迁移 → 自检 | 每阶段有超时和取消点；取得的资源始终有外层所有者 |
| 6 | 构造必要状态、配置读端、监督器；登记 ticker | ticker 在目标 runtime 初始化 timer，发送一次启动确认，然后等待提交门 |
| 7 | 等待 ticker 确认，同时监听信号、deadline、任务退出 | 确认/提交门通道关闭视为失败；先读当前 phase 再等变化，不丢失提前到达的提交 |
| 8 | 最后登记 HTTP；它在目标 runtime bind，回报实际 `SocketAddr`，然后等待同一提交门 | bind 错误由真实任务结果返回；不重试，不吞成一条日志 |
| 9 | 所有确认已收到，确认无已就绪的任务退出/停止请求；app 发布 Running | 此发布同时打开 ticker 工作和 HTTP accept 门；不是只改一个 readiness bool |
| 10 | 协调器持续选择信号、任务退出和配置重载结果 | 任一 first-failure 立即进入统一停止路径 |

启动确认采用两个具体的 oneshot：ticker 的 `()`，HTTP 的实际地址；不为两个面构造注册框架。
提交门复用只读 lifecycle 状态：只允许 Booting → Running 后开始工作，期间 child token 取消则退出。

绑定 socket 后，操作系统可能已处理 TCP 握手；本设计保证的是**启动提交前不 accept / 不执行业务 handler**，
不是保证外部探测连 TCP 都绝对连不上。

“全部确认”也不能保证任务未来不失败；它保证初始化被实际执行过。提交后的失败由监督循环处理，
不以“已启动”掩盖后续故障。

**取消安全要求**：不能在最外层对一个拥有半装配资源的 `boot()` 直接 `timeout` 后丢弃。
资源由协调器外层的 BootResources 持有，阶段 future 只借用资源；连接准备对象先登记，再 await 初始化。
阶段取消后仍由同一个所有者执行清理。若 deadline 到期导致第三方内部操作无法证明收敛，应标记不完整并退出，
不能凭 Drop 宣称清理完成。

### 5.5 信号和停止请求

- Unix：SIGINT、SIGTERM 进入关停，SIGHUP 请求配置重载。接收器一次创建并持有，不在循环内反复订阅。
- 非 Unix：至少支持 Ctrl-C；无 SIGHUP 时明确不提供 OS 热重载触发器，但配置发布/消费契约仍可用注入事件测试。
  不为跨平台补齐而偷偷增加第四个 HTTP 端点或常驻文件监视器。
- 控制器把 OS 输入归一成内部停止/重载事件；测试直接注入事件，OS 行为在子进程中验证。
- 停止阶段再次收到停止请求：立即跳到 Forcing，不刷新任何 deadline。
- 同步 runtime 销毁阶段已经不运行异步信号循环；第二次 Ctrl-C 不承诺在该阶段瞬时强杀。
  该阶段只使用剩余有界预算；需要绝对进程截止由外部进程管理者执行强制终止。
- 信号源意外关闭必须作为控制路径故障处理，不把 `recv() == None` 当成无限个正常信号。

### 5.6 关停预算与顺序

第一次停止观察时间为单调钟 `t0`。生产中的截止时间以 `std::time::Instant` 保存，
异步等待转换为 Tokio Instant，同步 teardown 沿用原截止时间，不能在 `block_on` 返回后重新计时。
四个正数预算是冷配置，做 checked arithmetic，拒绝溢出：

```text
D_grace   = t0 + grace
D_abort   = D_grace + abort_reap
D_storage = D_abort + storage_close
D_final   = D_storage + runtime_shutdown
```

每个阶段实际 deadline 为其固定边界与 `now + 本阶段预算` 的较小值；提前完成就推进，不空等。
未使用的早期预算不成为后续阶段的无限借款。没有任何 `N 个任务 × 一个完整宽限` 的算法。

| 阶段 | 操作 | 到期行为 |
| --- | --- | --- |
| 撤销服务 | 发布 Draining，再取消根 token；禁用新配置提交，停止发起新工作 | 两个操作间不 await；API 复查生命周期 |
| 协作收割 | 一次取消全部面；在 `D_grace` 前消费所有退出，包括已经完成的任务 | 保留未结束名字，切 Forcing |
| 强制收割 | 对剩余任务 `abort_all()`；只等到 `D_abort`；同时处理重载作业的收尾 | 输出 Aborted / Unreaped，停止等待，不写无界 drain |
| 关闭存储 | 仅在使用者已确认排空时，显式 `StorageOwner::close()`，最多到 `D_storage` | close 超时记清理失败；不重试整段宽限 |
| 关闭 runtime | 退出 `block_on`，按 extra 逆序、main 最后逐个 `shutdown_timeout(remaining)` | 进入该阶段只计算一次 `min(D_final, now + runtime_shutdown)`；所有 runtime 共享这个截止时间 |
| 进程退出 | 合并首因和清理报告，返回 ExitCode | 不留下继续服务或后台重试的路径 |

**存储关闭的证明门槛**：自有使用者已 join，且 HTTP 已正常返回、确认 graceful 排空；
启动未提交、HTTP 尚未通过提交门的失败路径则可直接证明不存在请求使用者，也允许显式关池。
不能只因为最外层 `JoinHandle` 已被 abort，就推断连接 handler 已结束。
若 HTTP 被强制取消/发生无法证明子任务清理的 panic，或仍有未知使用者，跳过“正常 close 成功”的路径，
保留资源随 runtime/进程收尾，报告不完整、非零退出。强制路径不承诺业务 flush、无损请求或立即释放所有 DB 资源。
详见 §9.5。

重载文件作业不使用数据库，但它也必须有所有者、在报告中出现；若其阻塞部分未返回，不得宣称整个应用任务集已收割。

`shutdown_timeout` 的含义是限制等待，不是杀死还在运行的阻塞代码，也没有逐线程完成结果。
日志用 `runtime_shutdown_returned`，不直接写“所有线程 stopped”；已知未收割、关闭阶段用尽预算均保守记失败。
进程级测试验证外部看到的退出时限，但不能把一次测试结果推广为任意恶意/不可中断代码的硬保证。[R2][R3]

### 5.7 子任务边界与退出码

协调器自身不是 JoinSet 条目，因此它的 panic 不能靠子任务监督器兜住。
app 的资源 guard 在异常 Drop 时尽力先撤销 readiness，再取消根 token，由 JoinSet 请求 abort；这仍不等于异步清理完成。[R10]
同步 main 在 `block_on(run)` 外设置一个窄 panic 边界：仅把协调器 unwind 变成“无法证明排空”的失败退出，
随后销毁 runtime，不尝试恢复或继续运行该服务。RuntimeSet 与最终截止时间上下文保留在该边界之外，
已经建立的关闭期限不能因 panic 刷新；尚未进入停止路径时只给一次应急 runtime 关闭预算。
这不覆盖 double panic、abort、OOM 或不可返回的代码；普通错误仍走 Result，不靠 panic 控制流程。

当前 ticker 不 spawn 子任务；HTTP 和 sqlx 的库内任务由各自库的生命周期协议管理。
以后一个面真正需要并发时，必须在同一次改动中说明子任务的 join 所有者、关闭顺序和预算：
父任务正常返回前收割其子任务，父任务异常 Drop 只请求取消，不能假报确认完成。
内部子任务的宽限必须短于顶层面可用宽限，且从同一个绝对关闭计划推导，禁止逐层刷新 timeout。

| 最终结果 | 退出码基线 |
| --- | --- |
| 主动停止，面协作排空、存储显式 close、无已知清理失败 | 0 |
| 配置、runtime 构造、连接、迁移、bind、启动确认失败 | 1 |
| 常驻面意外结束、错误、panic、非预期取消，或被最外层边界捕获的协调器 panic | 1 |
| 强制 abort、任何未收割、close 超时、runtime 清理预算耗尽 | 1 |

OOM、进程 abort、不可返回的 FFI、操作系统强杀不在 graceful 保证内；不同平台的原生信号退出码也不能伪装成上述正常返回码。
若项目后来选择 `panic=abort` 或运行不可终止计算，必须修改故障模型和验收条件，不能只改 Cargo profile。

---

<a id="section-6"></a>

## 6. 横切能力准入清单

| 能力 | 本版裁决 | 最小落点与理由 |
| --- | --- | --- |
| TaskSupervisor / 根取消 / deadline | 内置 | app 私有；服务能停的必要条件 |
| 可选 runtime 绑定 | 内置、默认未配置 | 仅 app 解析和装配；未启用无额外任务/线程 |
| 配置文件加载 / watch | 内置 | app 写，ticker 读；只有周期具备真实热消费者 |
| Lifecycle 快照 | 内置 | 启动提交、API readiness、关停是明确消费者 |
| Arc<dyn Storage> / 迁移 | 内置 | 启动自检与 ready 使用，fake 支持 Router 测试 |
| tracing / 请求 trace | 内置 | app 唯一初始化，router 一处内联 layer；不增加日志格式选择器 |
| 请求时间上限 / DB 探测上限 / body 上限 | 内置 | 都有具体资源保护作用；不把请求超时当作整条连接的关闭期限 |
| 三个系统端点、404、405、错误形状 | 内置 | 端点存在就必须兑现完整 HTTP 契约 |
| **提取器包装** | **唯一预置例外** | `api/src/extract.rs` 包装 Json / Path / Query 的 rejection，防止第一个取参 handler 无声破坏既有 JSON 错误契约；不猜任何业务 DTO，不新增依赖；必须带失败测试 |
| BuildInfo | 内置 | 第一条业务启动日志和 info 的真实消费者；不生成新 build-info 服务 |
| testkit crate | 暂不引入 | 先本地测试，真实重复再提取；不是按旧目录恢复 |
| metrics / EventBus / retry 框架 | 不引入 | 当前只有一个 ticker，结构化日志足够；不造无消费者的全局容器 |
| reconcile / UnitManager / TaskFactory | 不引入 | 无动态期望集、无多实体生命周期；加它就是预建框架 |
| CORS / 认证 / TLS / OpenAPI / Web UI | 不引入 | 没有浏览器跨域、身份、证书或公开 API 文档需求 |
| 通用 `repo` / 事务 trait / 业务表 | 不引入 | 首个真实垂直切片再设计 |
| 容器 / 交叉编译 / glibc 检测 / 固定端口 | 不引入 | 项目和部署事实，不属于模板正确性的证明 |

### 提取器例外的约束

承诺的范围是“遵循模板导入规则的路由所生成的应用错误”，不是所有网络层错误。
所有自有 handler 必须从本地 `extract` 模块导入 Json / Path / Query；模板维护脚本或代码审查规则检查绕过的直接导入。
包装模块的 axum 内部导入是明确例外，测试 fixture 也应显式标注。

包装沿用 `rejection.status()`，不把 400/413/415/422 拍平成 400。
5xx rejection 只在受控日志中记录内部原因，对外统一泛化；4xx 也只返回安全摘要，不原样回显整个 body、字段值或底层错误链。[R6]

如果包装尚无生产调用导致 dead-code lint，允许仅在这个模块上做带理由的局部豁免，
不能为通过 clippy 把整个 crate 的 unused 警告关闭，也不能为了消警告添加一个示例业务端点。

---

<a id="section-7"></a>

## 7. 三段式配置与热重载

### 7.1 先定义配置的真值与热度

**当前实现注记（M3）**：`Config` 已拆成 app 的 `BootConfig` 和 core 的 `HotConfig`；core 的生命周期 watch 与配置 watch 是两种独立能力。
主进程保留同一个 LoadContext（路径、目录、环境快照、默认 worker 数），SIGHUP 经单飞作业加载候选值，只有协调器能提交热快照。启动提交前和关停期间忽略重载请求。

配置来源只有一份 TOML 文件。路径选择优先级：

1. 显式 `--config <path>`；
2. 由生成器派生的 `<ENV_PREFIX>_CONFIG`；
3. 生成项目中的 `./config/service.toml`，仅作为从项目根目录运行的开发便利。

默认路径常量及纯选择函数属于 app 的 config 模块；main 捕获 CLI/环境/cwd 并注入，启动仅记录 `config_source` 安全类别，不回显原始路径/环境值。
选择后一次性绝对化，重载继续读取同一个路径；数据相对路径以**配置文件所在目录**为基准。
不以二进制位置推导安装根，不写入 `/etc` 或 `/var`，不随运行中 cwd 改变漂移。
正常启动缺文件明确失败；不自动创建或覆盖配置。生成器已经提供开发配置，初始化文件不是启动的隐式副作用。

```text
Candidate
├── BootConfig（冷）
│   ├── runtime：主/extra 的参数
│   ├── bindings：HTTP / ticker 目标
│   ├── http：监听、请求预算、ready 探测预算
│   ├── storage：SQLite 路径、连接数与超时
│   └── lifecycle：启动、协作收割、abort 收割、关池、runtime 预算
└── HotConfig（热）
    └── ticker.interval_ms
```

`Candidate` 与文件加载属于 app；`HotConfig` / `ConfigSnapshot` / 读写 handle 属于 core，供 app 和 worker 共享。
`BootConfig` 不经 watch 发布；不存在“已经把新端口发给大家、实际监听仍是旧端口”的混合快照。
其中 HTTP 参数类型由 api 定义为 `HttpSettings`，存储参数类型由 storage 定义为 `StorageConfig`；
app 持有 RawConfig 并转换/组合这些具体参数，api/storage 不反向依赖 app。runtime 绑定字段只留在 app。

**模块归属判据是“配置装配归 app”，不是“配置一律归 app”。** 配置来源、整份配置组合、跨组件校验和重载决策
属于装配层；组件自身的参数及约束随组件；确实跨 crate 共享的热配置契约放在 core。
保持组件参数类型的当前归属时，若把组合它们的 BootConfig 放进 core，就会引入 core 对 api/storage 的反向依赖；
另一种可行架构是把全部配置类型集中定义在 core，但那会让 core 承担整份服务 schema，不是本版采用的边界。
ConfigPublisher 定义在 core、唯一实例由 app 持有并决定何时发布，两者并不矛盾：类型定义位置和实例所有权是两个维度。

最小开发配置草案如下；未写 runtime 段即只有主 runtime，数值预算使用校验过的缺省值。
`0` 只用于本地动态端口演示，不定义部署端口：

```toml
[http]
listen = "127.0.0.1:0"

[storage]
path = "../data/service.sqlite3"

[ticker]
interval_ms = 1000
```

### 7.2 三段式管线

**M2 实测细节**：TOML 1.1 使用 `toml::from_str` 解析文档，不用解析单值的 `Value::from_str`；配置文件实际读取和环境展开后的字符串总量分别限制为 64 KiB。
打开前后都拒绝非普通文件，避免普通 FIFO 路径阻塞打开；这不构成对文件路径恶意替换竞态或卡死文件系统的硬期限保证。

| 阶段 | 输入 → 输出 | 具体规则 |
| --- | --- | --- |
| I. 读取与解码 | 配置文件 + 固定环境快照 → RawConfig | 只接受常规本地文件、限制读取字节数；先解析 TOML 值树，再对字符串值作环境展开，最后反序列化，未知字段报错 |
| II. 补全与规范化 | RawConfig + 配置目录 → Candidate | 缺省值、相对路径绝对化、数值/地址转换、runtime 名称解析、主线程数推导；无消费者就不留空 complete hook |
| III. 校验与定型 | Candidate → ValidatedConfig | 检查范围、交叉预算、额外 runtime 引用、所有权约束；只输出合法值，不在校验后再 sanitize 改写 |

这三段替代源项目散布的 resolve / complete / validate / sanitize 隐式约定：
**所有推导在校验前完成，发布后不再改值**。默认值在实现中只有一组常量/构造逻辑；
生成的示例 TOML 与省略字段后的补全结果必须有等价测试。

环境语法只允许字符串值中的 `${NAME}` 和 `${NAME:-fallback}`，一次展开、不递归、不运行 shell；
`$$` 表示字面 `$`。未定义且无 fallback、非法名字、需要 UTF-8 却不是 UTF-8 都报有字段路径的错误。
数字与布尔字段保持 TOML 类型，不把字符串占位符隐式转成数字；例如 ticker 周期写整数，不能假装任何字段都支持环境替换。

先解析再展开可避免密码中的引号、换行变成 TOML 语法注入，也避开 cargo-generate 的 Liquid 花括号。
测试注入环境查找函数；生产在启动时取得不可变 LoadContext（环境快照、配置目录和缺省 worker 数），
重载复用它，不重新受 cwd/CPU 探测结果影响，不在测试中 `set_var` / `remove_var`。
解析错误对外只显示字段路径和安全摘要，不打印整份配置、展开结果或原始环境值。

配置数值下界/上界在实现时逐项列为常量并测试，至少包含：

- ticker 周期正数且不会造成零间隔忙循环；
- worker / blocking 上限为正数，配置的 extra 数和累计预算有显式上限；
- SQLite 池连接数大于零，min 不大于 max，获取/锁等待超时有界；
- readiness DB 超时短于请求超时，请求超时短于正常 HTTP 排空宽限；
- 所有生命周期时长可安全转换、相加，不溢出单调钟期限。

上述上限是可编辑的模板资源安全策略，不是硬编码到不可变框架里的部署配额。

### 7.3 热发布接口与版本一致性

```rust
// core：只表达内存里的热配置，不负责读文件。
struct ConfigSnapshot {
    generation: u64,
    config: HotConfig,
}

struct ConfigPublisher {
    tx: tokio::sync::watch::Sender<Arc<ConfigSnapshot>>,
}

#[derive(Clone)]
struct ConfigHandle {
    rx: tokio::sync::watch::Receiver<Arc<ConfigSnapshot>>,
}
```

- publisher 不 Clone，由 app 协调器唯一持有；提交用 `&mut self`，不需要第二个 atomic generation。
- 初始代号为 1；只有有效热值变化才 `checked_add(1)` 并发布；代号耗尽拒绝重载，不能绕回 0。
- generation 和值同处一个 Arc，`send_replace` 一次替换。没有活跃 receiver 时仍保存最新值。
- `current()` 只克隆 Arc 并立即释放 borrow；`changed()` 先等待变化，再 `borrow_and_update()` 克隆。
- 不跨 await 持有 watch Ref；watch 可以合并多次发布，代号跳跃合法，不承诺每个消费者逐次执行中间值。
- writer 意外消失而根 token 尚未取消时，ticker 返回错误触发监督；关停期间则正常退出。[R5]

### 7.4 重载事务与非阻塞协调器

**M3 实现落点**：`app/src/config/reload.rs` 持有一个 JoinSet 作业槽和一位 pending 标记；`lifecycle.reload_timeout_ms` 默认 2000ms、范围 (0, 60s]，本身属于冷配置。
超过请求期限会先撤销结果的发布资格，但不会释放未结束的作业槽。协调器在循环入口检查期限，避免连续重载事件压住 timeout 分支。
冷字段报告按类型比较：HTTP/Storage 使用组路径 `http` / `storage`，其他当前冷量列具体路径；不输出值，不在字符串白名单未匹配时默认放行。

重载流程：

```text
SIGHUP / 测试事件
  → 单飞文件作业执行完整三段式管线
  → 回到 app 协调器，确认仍是 Running、未超时
  → 与启动时保存的 BootConfig 比较
      ├── 冷字段不同：RejectedRequiresRestart，整批不发布
      ├── 完全相同：NoChange，代号不变
      └── 只有 HotConfig 不同：一次发布，Published { generation }
```

冷热比较使用类型化相等判断和明确字段列表，不做“字符串前缀没匹配上就默认可热”的递归猜测。
新字段默认属于冷配置；新增热字段必须同时有消费逻辑、发布契约和测试。
含冷、热两类变更的文件整批拒绝；磁盘保持原样，运行中仍使用旧配置，报告列出需要重启的字段路径但不列值。
修改文件属于操作者职责，模板不在重载失败时写回旧配置。

重载文件读与解析不能在协调器循环内同步阻塞，也不能 `await reload()` 到屏蔽停止信号：

- app 使用一个局部持有的 blocking 作业槽（JoinSet 或等价 join 所有者），最多一个 in-flight。
- 重复请求合并成一个 pending 位，不积累无界队列；作业结束后至多再读取一次当时的最新文件。
- 作业有请求 deadline，超时后本次结果永不发布，但槽位必须等原作业真正返回才释放；不能 timeout 一次就再 spawn 一个堵住的线程。
- 正在读的普通文件发生 I/O 卡死时，Tokio abort 不能杀掉已经开始的 blocking 代码；仍保持槽位占用、可接收关停，最终按 §5 报告未收割。
- 关停先封住唯一提交点；即使读任务稍后成功，也必须丢弃结果，不得关停后再增代。
- 预期 I/O / 解析 / 校验错误只拒绝该次重载；loader panic 是代码缺陷，升级为控制路径故障并关停。
- 文件读取限制必须作用于实际读取过程（例如至多上限加一字节），不能只凭 metadata 的长度检查。
  原子 rename 是推荐的写文件方式，但不宣称能锁住外部写者；读到语法错误保持旧配置，读到合法快照则按该快照校验。[R3]

“Published”只说明读端可见的新快照已经提交，不谎称所有消费者已完成动作。
当前唯一消费者 ticker 在下一次循环观察到后记录自己的 `config_generation`。
不为了等待一个 ticker 加配置 ACK 注册表或另开配置查询端点。

### 7.5 ticker 的消费语义

**M3 实测修正**：Tokio Interval 的 `reset_at` 只改下一次 deadline，不改变后续 period。实际周期变更要重新构造 interval 并设置 Skip；启动提交后也按最新快照重新构造。
相同已消费代号不重复重置。`config_reload` 发布日志和 `ticker_config_applied` 消费日志可能跨线程先后交错，测试使用共同日志检查点，不假定发布日志一定先打印。

1. 在目标 runtime 内初始化 timer 并发送启动确认；观察到 Running 后，重新读取最新快照，
   将 timer 重置为“观察 Running 的时刻 + 周期”。不能让启动门前的等待消耗掉首周期。
2. 第一次 tick 在一个完整周期后发生，不利用 `interval` 的默认立即首 tick 当作工作或就绪证明。
3. 循环同时等待取消、配置变化和 tick；取消优先。每个分支的工作有限，不做无界同步计算。
4. 观察到新周期后从“观察时刻 + 新周期”重新设置下一次 tick，丢弃旧 deadline，不额外补打一拍。
5. 周期错过采用 Skip，不逐一补发全部遗漏拍；这不保证任意相邻两拍都相隔完整周期，
   也不消除底层 timer 的容差。记录 tick 序号和已消费代号，不上报伪精确的墙钟间隔。[R11]
6. 使用单调时间计时，墙钟仅日志展示；不引入共享计数器或数据库写入。

---

<a id="section-8"></a>

## 8. 单后端存储、迁移与 fail-fast

### 8.1 边界与最小门面

本版提案只编译 SQLite。没有 `StorageBackend` enum、动态 DSN 分派、两个后端目录、成对迁移或 backend feature 矩阵。
把 SQLite 换成 PostgreSQL 是一次明确的代码/配置/测试替换，不是本版的隐藏配置开关。

```rust
pub type StorageFuture<'a, T> =
    Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Storage: Send + Sync {
    fn health(&self) -> StorageFuture<'_, Result<(), StorageError>>;
}
```

health 由 ready 消费。关闭能力不在 `dyn Storage` 上，而在 app 独占的 `StorageOwner::close` 上：
API 持有的门面无法顺手关掉共享池。测试替身也只需实现 health。
不先放 CRUD、事务回调、模型、分页或默认返回 `Unsupported` 的方法。

使用显式 boxed future 是为了在不引入自研宏或额外 async-trait 依赖的情况下获得可用的 `dyn Storage` 边界；
不能直接假定 trait 上原生 `async fn` 就可转成 trait object。动态分发/装箱只发生在门面调用，不为所有内部函数泛型化。[R9]

`StorageOwner::close` 是普通 async 方法，只有开始轮询才发起实际关闭；
app 的清理函数只在使用者排空后调用它，测试可注入一个受控关闭 future，不为此新增生产管理 trait。

`StorageError` 公共签名不出现 sqlx；保留安全的阶段分类和私有 source 供受控诊断。
API 按当前 health 的结果生成 503，不通过 downcast 数据库错误链推断 HTTP 404 / 409。
等首个业务仓储出现时，再随真实语义添加 typed errors。

### 8.2 可取消的启动与存储关闭所有者

只有一个 `init_storage().await` 黑盒很容易在外层超时后丢掉仍需关闭的池，因此设置一个具体的启动中间态：

```text
storage::prepare(validated_config)
  → StorageOwner（app 立即持有；内部持有尚未发布的池）
  → initialize：连通检查 → 迁移 → 自检
  → 成功时返回 Arc<dyn Storage>
```

`prepare` 可以在 storage 内部使用 lazy pool 构造来做到“先记录所有者、后 await”，
但**不允许**把这一步返回解释为存储已可用。池从未经过三项检查前，不向 handler 发布其门面。
`StorageOwner` 只提供初始化和关闭所需的具体方法，不暴露 sqlx 类型，也不是多后端工厂接口。

初始化 future 借用这个外层对象；超时/取消后对象还在，app 仍可限时调用 close。
成功后 app 也继续保留 owner，API 只拿返回的门面；不把“初始化完成”误写成所有权转给 HTTP。
迁移、自检失败也走同一路径，不靠 Arc Drop 当作已断开所有连接的证据。[R4]

启动 fail-fast 的确切含义：

- 文件父目录创建/写入权限、数据库打开失败、池获取超时、迁移锁等待超时、迁移不匹配、自检失败，均不提交 Running。
- 不进行无界应用级重试；数据库自带的有界锁等待必须包含在启动 deadline 内。
- SQLite 外键等连接级选项在每个连接建立时应用；按真实选项测试，不只初始化第一条连接。
- 正常应用路径不使用内存库；测试若用内存库必须显式处理连接共享语义，优先 tempfile 文件库。
- 用 SQL 查询完成 health，而不是只读取 `pool.is_closed()` 或缓存一个健康 bool。
- 没有业务表时只能证明连接、迁移状态和简单查询可用，不能宣称已验证未来所有业务 SQL 或任意写入权限。

### 8.3 迁移纪律

- 只有 `storage/migrations/` 一个目录；初始只有说明文件，没有示例业务表、假数据或空操作版本占位迁移。
- 始终执行嵌入式 Migrator，即使业务迁移集合为空；允许 sqlx 的 `_sqlx_migrations` 元数据，
  “不预建表”指不预建**业务表**。空集合和元数据行为要在所选 sqlx 版本上实测。
- 添加首个业务迁移时按单调编号命名，不修改已应用文件；禁止在 handler 中做 DDL。
- `build.rs` 追踪迁移目录，验证新增/修改/删除文件都触发嵌入集变化，不能只依赖已有文件的 `include_str`。[R4]
- 生成工程的 `.gitattributes` 固定 `.sql` 为 LF，并测试生成/跨平台 checkout 不改迁移字节；不靠忽略空白来放宽校验和。[R4]
- checksum mismatch、已应用版本缺失、dirty 或迁移执行失败均阻止启动；模板不自动改迁移元数据、不忽略失败、不自动降级。
- 迁移锁和事务边界依赖具体 sqlx/SQLite 实现，实施时必须用并发启动、进程中断、重启用例验证；不把 WAL 当作迁移锁。
- 停止信号在迁移中到达时走有界取消；**不承诺 DDL 全未执行**。下次启动重新验证数据库状态；恢复由操作者按已知迁移处理。

### 8.4 关闭和测试替身

sqlx 池的显式 close 会拒绝新的获取并等待借出的连接归还；它仍可能因使用者未归还而等待。
所以 app 必须先满足 §5.6 的使用者排空条件，再用剩余 storage budget 关闭。[R4]

Router 测试提供健康、失败、挂起三种最小 Storage fake；只有 storage 自身的测试使用 SQLx。
迁移成功/不匹配/新增文件测试使用测试专属 fixture 和临时目录，不把测试表放入生产迁移或生成服务 schema。

---

<a id="section-9"></a>

## 9. HTTP 契约与就绪语义

### 9.1 最小状态与接口

```rust
#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub lifecycle: LifecycleHandle,
    pub build: BuildInfo,
}

pub fn build_router(state: AppState, http: &HttpSettings) -> axum::Router;

pub async fn serve(
    state: AppState,
    http: HttpSettings,
    shutdown: CancellationToken,
    started: tokio::sync::oneshot::Sender<std::net::SocketAddr>,
) -> Result<(), ServeError>;
```

三项 AppState 字段都有消费者：ready、ready/启动门、info。
监听和超时是构建参数，不为每个 handler 注入整份配置；没有 ConfigPublisher、Metrics、EventBus、runtime、supervisor 或根 token。
`HttpSettings` 由 api 定义，包含已解析的 `SocketAddr` 与请求/探测期限，不包含 runtime 名称或文件路径定位逻辑。
`serve` 不 spawn 自己；bind 和 listener 的 I/O 注册发生在 app 指定的目标 runtime。

### 9.2 系统端点

| 路由 | 成功 | 失败/停止语义 | 依赖 |
| --- | --- | --- | --- |
| `GET /v1/service/health` | 200，无数据成功信封 | 能处理本请求即证明 HTTP 面存活；不因 DB 故障改成失败；停止时不保证还能建立新连接 | 不访问存储 |
| `GET /v1/service/ready` | 200，无数据成功信封 | 非 Running、生命周期写端已关闭、DB 错误或探测超时为 503 | lifecycle + 一次有界 health 查询 |
| `GET /v1/service/info` | 200，裸 JSON | 不触发 DB 探测，不暴露部署细节 | BuildInfo |

就绪判定：

```text
读取生命周期，写端已关闭或非 Running → 503
  → 在 ready_probe_timeout 内执行 storage.health()
  → 失败/超时 → 503
  → 再读生命周期，写端仍存活且仍 Running → 200，否则 503
```

第二次检查避免慢探测跨过 Draining 边界还返回成功；它的线性化点是最后一次状态读取。
不能保证 response 写到客户端那一瞬间状态仍未改变，这种分布式可见性不靠一个 bool 消除。

首次 Running 之前，storage 初始化与任务启动确认已经完成。运行中顶层任务失效由 supervisor 触发撤销 readiness，
不在每次 ready 请求里遍历全部 JoinHandle，也不预建任务健康注册表。

### 9.3 响应与安全契约

成功无数据与所有**应用能够生成响应的错误**共用一个类型：

```json
{"status":"success","code":200,"description":""}
```

```json
{"status":"error","code":503,"description":"service is not ready"}
```

- `code` 始终等于实际 HTTP 状态码；`status` 由同一个构造器生成，handler 不手拼。
- 成功带数据回裸 JSON；info 仅包含服务名、版本和可选构建 revision，不给配置路径、数据库路径、环境值或 runtime 拓扑。
- 两种成功形态由 `ApiResponse::{Ok, Data}` 命名：裸 JSON 是被声明的契约，不是「忘了包信封」。裸 `Json<T>` 表达不出这个区别，因此 handler 不直接返回它。契约的三条规则写在 api crate 与 response 模块的文档注释里，随生成项目交付；本节是设计侧记录，生成物不包含 `docs/`。
- 5xx 对外用有限、稳定的安全描述；原始 SQL、驱动错误、配置内容、用户输入不直接写入 response。
- 错误侧由 `HttpError` 枚举承担，状态码与安全描述集中成一张表，`error` 模块的表测逐变体钉死；handler 不能现场指定任意 (状态码, 文案) 组合。描述是 `&'static str`，没有插值的缝，因此「不把用户输入写进 5xx」是类型错误而不是纪律。503 与 500 的分界是「换个时间再来会不会好」：503 说依赖此刻不通、请求本身没毛病，调用方可以退避重试；500 说这条路走不通，重试多少次都一样。混用会让客户端的重试策略失去依据。
- 不在本版发明业务错误码注册中心；增加业务错误码属于首个真实 API 的契约设计。
- HEAD 按 HTTP / axum 语义没有响应体；不能把“HEAD 空 body”列为 JSON 契约失败。
- 错误信封的边界不覆盖 malformed HTTP、TLS、对端断开、响应已发送后的流错误和进程被强杀。

### 9.4 router 完整覆盖

在所有路由/fallback 构建完成后统一装配 layer，显式覆盖：

- `/v1`、`/v1/`、`/v1/unknown`、根路径和完全不在版本前缀内的路径；
- 已有 GET 路由收到 POST 等不支持方法时的 JSON 405，并保留框架正确的 `Allow` 语义；
- Json / Path / Query rejection，包括媒体类型、语法、数据形状、payload 过大及内部提取错误；
- 应用请求处理期限到期时的 JSON 503；本版不用返回空 body 的默认 TimeoutLayer 响应冒充统一错误格式；
- DB 探测自己的 503，必须比外层请求期限更早完成。

请求 timeout 用 router 装配处的简短内联 middleware 实现，统一调用 HttpError 转换，
不建 middleware 目录、不新增服务 trait。TraceLayer 放在它外侧，连超时响应也能记录。

提取器 body 限制沿用框架有限默认值并在包装测试中验证；不因三个 GET 端点预放可配置的业务上传大小。
后来新增流式 body 消费者必须自己提供大小、空闲、总期限及取消策略，不能误以为默认提取器限制覆盖所有读取方式。

TraceLayer 必须显式选用 method、匹配的**路由模板**、status、latency 等安全字段；
未知路由使用固定标记，不直接记录完整 URI/query/body/authorization。不能以“用了默认请求 trace”替代日志脱敏审查。

### 9.5 graceful 与强制终止的边界

正常路径使用 `axum::serve(...).with_graceful_shutdown(...)`：收到 child token 后停止接收新工作，
等待库管理的连接排空；`serve` 正常完成才成为 HTTP 已收敛的证据。[R6][R7]

**中止外层 serve future 并不递归 join/abort 它曾交给库的所有连接任务。**
因此，不得写“监督器 abort HTTP 后所有 handler 都已停”。强制路径必须：

1. 标记 HTTP 未获得 graceful 完成证明，不恢复 Running。
2. 不开始需要“所有 handler 已停”前提的正常存储关闭/最终业务 flush。
3. 对所有 runtime 做剩余预算内的关闭，最终退出进程；由 runtime/进程结束终止仍存活的库内工作。
4. 返回非零，并在总结日志保留 HTTP 未排空的事实。

请求处理 timeout 不覆盖所有慢 header、长连接和流式 body；库内 accept / 单连接错误也不必都表现为顶层 future 返回 Err。
同理，单个 handler panic 发生在库管理的连接任务内，不必导致 HTTP 顶层面退出；本版的 first-failure 不冒充请求级 panic 监督。[R7]
本版不承诺只靠三个系统端点的普通请求测例，就证明任意协议扩展都能限时无损停机。

如果未来要求“到期限逐连接强制关闭后，仍在同一进程内安全复用数据库并继续运行”，
必须重新设计持有每条连接 JoinHandle 的 HTTP 驱动，或选择有这种所有权 API 的服务封装。
那是明确的新需求，不在这个最小模板里悄悄引入一套 Hyper server 框架。

---

<a id="section-10"></a>

## 10. 错误处理与可观测性

### 10.1 错误边界

| 来源 | 在哪一层分类 | 结果 |
| --- | --- | --- |
| CLI / 配置读取与校验 | app | 字段路径 + 安全解释；启动失败或重载拒绝 |
| 数据库驱动 / 迁移 | storage | 保留内部 source；启动错误上交 app，ready 的 health 失败转 503 |
| ticker | worker | 明确返回错误，不只 log 后 `return ()` |
| HTTP bind / 启动门 / serve | api | 具体错误交 app，不吞错误 |
| 顶层 panic / 取消 / 正常提前结束 | app supervisor | 命名任务退出；first-failure |
| 关停耗尽 / close 失败 | app shutdown | 追加到 ShutdownReport，不覆盖原始原因 |

StopCause 固定保存第一个停止原因；ShutdownReport 同时列清理问题和最终退出码依据。
例如“启动 bind 失败，随后关池超时”的首因仍是 bind，不能只留下 `deadline exceeded`。

不在多个层级对同一个错误反复 `error!`；边界负责补上下文，最终所有者负责记录。
SQLx 或 panic 的原始文字可能包含敏感数据，内部日志也不是密钥保险箱；只记录经过白名单处理的安全摘要和错误类别。

### 10.2 panic、全局设施与日志

- tracing 只在 app 的进程入口初始化一次；其他 crate 不调用 `init()` / `try_init()`。
- 模板采用固定的 compact 单行结构化字段日志，写入 stdout，不带多个日志格式开关；`RUST_LOG` 是唯一日志过滤环境入口，启动读取，不热更新。
- 仅 stdout 为终端时自动启用 ANSI 样式；非空 `NO_COLOR` 或 `TERM=dumb` 禁用样式，空 `NO_COLOR` 不禁用。文件/管道始终纯文本。
  颜色策略在 app 启动时捕获，只改变呈现，不改变事件/字段、日志过滤或配置热重载契约；不新增配置字段或依赖。
- 全局 subscriber 和 OS 信号处理是进程基础设施例外；它们不是业务状态单例。集成装配函数本身不安装它们。
- 两实例测试使用私有 subscriber 或显式注入捕获设施；信号、CLI、panic 的进程副作用只在子进程验证。
- release 保持 unwind：任务 panic 才可能成为 JoinError 被监督器看到。默认 panic hook 可能先写 stderr，
  模板不为了消除这行输出安装全局“吞 panic”hook；日志采集需要同时采集 stderr。[R8]
- 不把 `catch_unwind` 当成内存损坏、abort 或 FFI 故障的恢复手段；模板不承诺这些情况还能执行析构。

### 10.3 最小观测字段

| 事件 | 必须有的字段 |
| --- | --- |
| 进程启动 | service、version、可选 build revision |
| runtime 构造 | runtime、worker_threads、max_blocking_threads、bound_tasks |
| 启动确认/提交 | task、runtime、phase；HTTP 的实际监听地址仅在受控启动日志中出现 |
| 配置重载 | result=published/no_change/requires_restart/invalid/timeout、generation、变更字段路径，不含值 |
| ticker | tick_seq、config_generation、interval_ms |
| 顶层退出 | task、runtime、phase、kind、脱敏错误类别 |
| 请求 | method、matched_route、status、latency，不含完整 URI 或任意 header |
| 关停总结 | cause、elapsed、graceful/aborted/unreaped 名单、storage_close 结果、runtime 等待结果、exit_code |

不输出“服务已就绪”后才去 bind；不输出“应用已全停”后还进行无限 drain。
无指标服务、无指标端点；将来接 metrics 时使用实例化 registry，不创建业务 static。

---

<a id="section-11"></a>

## 11. 模板生成与工程链复用

### 11.1 复用白名单与必须改造项

以下都是**实施阶段的复用计划**；本次没有从备份复制任何工程文件。

| 备份工程件 | 可保留的思想 | 必须重新审查/改造 |
| --- | --- | --- |
| `cargo-generate.toml` | 项目名与 crate 前缀分离；模板私有文件不进入生成结果 | 五 crate 清单、Liquid 排除列表、新配置语法、生成项目 CI 来源 |
| `hooks/pre.rhai` | 从项目名派生环境前缀，校验包名前缀 | M1 改用稳定 Rust 依赖别名，不再生成源码导入名变量；保留长度/合法字符校验和确定性推导 |
| `hooks/post.rhai` | 将 `.project` 版本重命名并清理模板专用文件 | 移入正确的项目 CI；hook 仅改生成目录，不执行任意外部命令 |
| `Makefile` | 在树外生成，模板源码不直接运行 cargo；生成结果做 fmt/lint/test | 写操作不能在同一个 GEN_DIR 并发；默认命令无破坏性覆盖 |
| `Makefile.project` | 简洁的 build/run/fmt/lint/test/check | 所有需要解析依赖的 gate 使用 `--locked`；参数转发文档化 |
| 名称格式矩阵脚本 | 不只对一个短名字 fmt-clean | 同时验证生成后的 manifest、导入名、环境前缀、二进制名及残留占位符 |
| `template-sync.py` | 只保留“格式化生成物后回写”的需求，不恢复旧替换实现 | M1 由 `scripts/template.py` 按 Rust 文件原样回写；源码含零命名占位符，无反向替换；回写前拒绝覆盖期间已变动的源码 |
| `Cargo.lock` 回写流程 | 生成项目可直接 `--locked` | M1 仅映射 workspace 身份，第三方名称/校验和不改；依赖引用补足版本/source 以消歧，实际验证了本地 sqlx-core 与 registry sqlx-core 同名的情况 |
| 模板 CI | 权限最小、并发去旧、clean-room 生成验证 | 删除 PostgreSQL service/变量/跳过策略；加入生命周期、release、多 runtime 和单后端 feature 验证 |
| 各 crate README | 边界 / 目录 / 决策 / 测试四段 | 所有内容按新实现写，不带旧业务名和已删除抽象 |

不恢复旧 `Cargo.toml` 的业务依赖、旧数据库迁移、旧 `panic=abort`、旧外部案例文本或部署脚本。
旧 lockfile 中的第三方版本也不是免审计资产；M1 先验证新五 crate 的依赖组合，再固定工具链和锁文件。

### 11.2 生成产物与配置约定

- 保留 `project-name` / `crate_prefix` 两个概念，其他名字确定性派生；生成器不询问后端、多 runtime 或业务能力开关。
- 生成项目包含五 crate、自己的 Cargo.lock、简洁 Makefile、README、开发配置和调用项目门禁的 CI。
- 模板自己的 CI 不原样复制：M1 使用 `project-ci/workflows/ci.yml` 为源，post hook 将整个 `project-ci` 目录移为 `.github`，无需 hook 创建中间目录；
  Liquid 仅处理 manifest/锁文件和项目 README 白名单，Rust 与项目 CI 原样复制；生成门禁按字节验证没有放错 workflow。
- 模板调研文档、生成/回写脚本、hooks 和 `.project` 中间文件不留在生成项目。
- 生成物不能要求使用者了解实施阶段或未随项目交付的设计章节。源码注释解释当前约束、原因和失败边界；文件/线程/日志/夹具按用途命名。
- `make gen` 和结构门禁都会执行输出审计；非渲染文件与源按字节对应，渲染文件审查原始模板，避免把用户合法的项目身份误判为历史标签。
- release panic 验证随生成项目保留在 `app/tests/fixtures/release_panic.rs`。Cargo 的 `[[example]]` 目标仅提供独立 release 编译入口（`test = false`、`bench = false`）；不放 `app/examples`，也不把它描述为服务使用示例。
- 默认缓存根、受管目录前缀和报告字段采用用途命名；旧受管目录只有在显式指定原生成根且所有权匹配时才可清理，不自动搬迁或覆盖。
- README 必须给“生成 → 运行门禁 → 启动 → 根据日志的实际地址访问三个端点 → Ctrl-C”最短闭环；
  ticker 删除方法、第一条迁移、可选 runtime 绑定和热重载失败语义分别有短说明。

开发配置可以使用 loopback + 操作系统分配的临时端口（`127.0.0.1:0`），
这里的 `0` 是“申请临时端口”的 API 语义，不是模板替服务选定部署端口。
进程启动日志回报实际地址。真实监听地址、对外暴露范围和固定端口由生成项目设置。
SQLite 数据路径也只是一份相对配置目录的开发示例，不代表安装布局。

### 11.3 两类门禁

**生成项目基础门禁：**

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

另有真实 release 二进制的生命周期验收；不能用普通 `cargo test` 的测试 profile 代替它。
生成项目 CI 至少应覆盖 debug 基础门禁、release 构建和 release 进程退出测试。

**模板维护门禁：**

1. 生成唯一的树外临时目录，验证名字、五 crate 边、单后端 feature 闭包和模板残留。
2. 在生成结果执行基础门禁，fmt 静态检查只作预筛，真实生成格式检查才是判据。
3. 用短/长/含连字符、项目名与前缀不同、前缀易与依赖名撞词的名称矩阵验格式和结构。
4. 至少另一组名字从空 target 完整生成、构建、测试；不能只复用同一增量缓存。
5. 执行 §12 的迁移新增文件重编译验证、信号/启动失败/双 runtime/release panic 进程测试。
6. 用实际 lock/toolchain/generator 版本作缓存键；工具安装固定可审计版本，CI 不临时漂移到 latest。

模板完整检查需要从该模板当前工作树生成，不能错误拉取远端 main 来测试。
`make -j check` 不能让多个生成/回写任务修改同一目录；要么明确串行化写阶段，要么每个矩阵项独占输出目录。
任何 clean / overwrite / sync 都先验证目标是本任务创建、带标识的生成目录，拒绝仓库根、家目录、根目录和未标记的已有工程。
M5 维护入口显式使用 `--no-workspace`，防止生成到另一 workspace 下时自动修改其 Cargo.toml；实际嵌套 workspace 与并行同名生成都有探针。fmt/lock 通过写锁串行，回写前验证全量源码快照；编辑器不参加锁，不宣称与外部写入构成原子事务。

首次平台验收以实际跑绿的平台为准；不能因为提供了 `cfg(not(unix))` 分支就宣称 Windows 已支持。
若扩展 Windows CI，可直接运行 Cargo 门禁，不把系统预装 GNU Make 当作默认前提。

---

<a id="section-12"></a>

## 12. 测试策略与验收矩阵

**下表是完整模板的验收要求，证据与范围逐项见 `docs/acceptance.md`。** M1–M4 历史记录保留；M5 新增生成矩阵、迁移 fixture/真实嵌入集合、慢连接和 handler panic 证据，当前执行结果见 `docs/verification.md`，阶段原始结果见对应历史报告。跨平台、任意业务迁移/负载不因本地门禁通过而自动验收。
每个行为至少有一个可重复的失败注入用例；所有预期超时都设置外层进程 watchdog，避免测试套件本身挂死。

### 12.1 分层方法

| 层级 | 方法 | 不做什么 |
| --- | --- | --- |
| 配置/状态机/预算 | 纯函数、表驱动、输入环境闭包、checked arithmetic | 不访问真实进程环境、不启动 Web 服务 |
| ticker/watch | Tokio paused time、oneshot/屏障、可观察快照 | 不 sleep 一小段后猜 tick 次数 |
| 监督器 | 可控 future、Drop 探针、panic、pending、真实 JoinSet | 不只断言发出过 abort |
| API | Router + `oneshot`、本地 Storage fake | 不需要真实 DB / TCP / TLS |
| 存储 | 每测试独立 tempfile SQLite、真实 Migrator、测试 fixture | 不共用固定数据库、不带业务种子表 |
| 多 runtime | 普通同步 `#[test]` 构建多个真实 multi-thread runtime（`app/tests/runtime_semantics.rs`，进程内） | 不在 `#[tokio::test]` 内直接 Drop 嵌套 Runtime；虚拟时钟不能证明跨 runtime 调度 |
| 进程与信号 | 子进程、OS 分配端口、启动握手、外层终止回收 guard（`app/tests/check_service.py` / `check_logging.py` / `check_release.py`） | 不在测试进程给自己发信号、不固定端口、不留下失败子进程 |
| 生成/工程链 | 临时目录、名称矩阵、clean-room 构建、manifest/feature 解析（`scripts/test_template.py` 与 `scripts/template.py`） | 不只 grep 出单词就当成依赖隔离成立 |

**执行入口不统一；`cargo test` 不是完整门禁。** 上表前六层由 `cargo test --workspace --locked` 覆盖。
"进程与信号"层由 `app/tests/` 下的 Python 驱动承载：`check_service.py`（5 种 runtime 拓扑 × debug/release，
覆盖端点与错误信封契约、SIGHUP 重载、慢首部关闭、客户端断开、bind 失败回滚、
配置/拓扑/存储路径/迁移的 fail-fast、存储初始化期信号）、
`check_logging.py`（PTY / 管道 / 文件三种 stdout sink × debug/release）、
`check_release.py`（构建并运行真 release 二进制，校验受控 panic 的退出码）。cargo 不识别这些文件，
只有生成项目 `Makefile.project` 的 `service-check` / `logging-check` / `release-check` 会运行它们；
"生成/工程链"层则由模板自己的 `Makefile` 经 `tooling-test` 与 `check` 运行，不在生成项目内。
因此只跑 `cargo test` 会得到一个不含任何进程级证据的全绿结果；生成项目的完整门禁是 `make check`
（`Makefile.project`），其 CI（`project-ci/workflows/ci.yml`）执行的也是它。
§12.2–§12.5 的 LIFE / RT / CFG / TICK / DB / HTTP / GEN 编号是文档编号，不出现在任何测试名或源码中；
定位用例需按上述入口与文件对照，不能靠搜索 ID。

### 12.2 生命周期与 runtime 验收

| ID | 场景 | 必须证明 |
| --- | --- | --- |
| LIFE-01 | ticker 尚未确认或 HTTP 尚未绑定 | 无 Running 提交；HTTP 不开始处理 handler |
| LIFE-02 | 延迟 ticker 初始化、延迟 HTTP 初始化 | 真实握手控制启动，不靠注册顺序猜就绪 |
| LIFE-03 | 存储各阶段错误、bind 失败、确认 sender 消失 | 无对外服务；已持有资源进入统一清理；首因保留 |
| LIFE-04 | 在连接/迁移/等待确认期间注入停止 | 取消可被观察，启动不再提交；资源所有者没有被外层 timeout 丢掉 |
| LIFE-05 | 任一常驻面意外 `Ok`、`Err`、panic、外部 abort | 名字/runtime/原因准确；readiness 撤销；非零退出 |
| LIFE-06 | 信号和任务退出同时已可读 | first-failure 不被普通停止结果覆盖 |
| LIFE-07 | 正常根取消 | 每个 child 收到，兄弟不能互相取消；自有任务都 join 后才关池 |
| LIFE-08 | 配合 poll、却不响应 token 的 pending 任务 | graceful 到期 → abort → Drop/JoinError 确认；不假报 graceful |
| LIFE-09 | 可释放闸门控制的 blocking 作业不返回 | abort 后仍有第二个 deadline；报告 unreaped，测试清理时释放闸门 |
| LIFE-10 | 空 supervisor、重复名字、停止后注册 | 无忙循环；release 也拒绝重名/新注册 |
| LIFE-11 | 第二次停止请求 | 提前进入 Forcing，不延长最终 deadline |
| LIFE-12 | 注入的 storage close future 挂起 | 有独立关闭上限，保留原始停止原因，最终非零 |
| LIFE-13 | HTTP graceful 排空 / HTTP 被强制 abort | 前者确认在途处理完成；后者不得把外层 join 当成所有连接已停止，不进入虚假的正常 close 路径 |
| LIFE-14 | 同一进程两套 app 装配与配置源 | 状态/代号/存储/token 不串；测试不依赖单线程执行 |
| LIFE-15 | 协调器自身 unwind | 根取消和 JoinSet 兜底执行；同步 main 仍有界销毁 runtime，非零退出，不尝试恢复服务 |
| RT-01 | 无 extra | RuntimeSet 只构建主 runtime，所有面直接登记，无转发任务或额外线程池 |
| RT-02 | ticker 绑定单 worker extra | 目标 runtime 内的线程/handle 标记匹配，timer 确实运行，主 runtime 可继续服务 |
| RT-03 | HTTP 绑定 extra | listener 在目标 runtime 创建；跨 runtime readiness 查共享池；正常关闭后主 runtime 最后退出 |
| RT-04 | 无效名字、main 名冲突、未引用 extra、预算越界 | 在启动资源副作用前拒绝 |
| RT-05 | 第 k 个 runtime 构建失败 | 已建 runtime 显式限时关闭，不触发 async Drop，不落入无界隐式 Drop |
| RT-06 | 多个额外 runtime 清理变慢 | 总等待使用同一剩余预算，不随 extra 数线性叠加完整宽限 |
| RT-07 | 真正 release 二进制中的受控任务 panic | 主动触发仅测试 fixture 的 panic，真实二进制仍走监督/清理并非零；不是仅测试 harness 的 panic 策略 |

不可终止的恶意 CPU 死循环只在可强杀的独立子进程测试，不能污染测试 runner。
测试结束后必须回收子进程；期望是“限制的故障边界被正确描述”，不是要求 Tokio abort 杀掉 OS 线程。

### 12.3 配置与 ticker 验收

| ID | 场景 | 必须证明 |
| --- | --- | --- |
| CFG-01 | 缺文件、非法 TOML、未知字段、错误类型 | 安全的字段路径错误，无默认掩盖 |
| CFG-02 | 相对配置路径、不同 cwd/CPU 探测结果、相对 SQLite 路径 | LoadContext 固定；文件未变不会因环境推导漂移而被判冷变更 |
| CFG-03 | 环境值含引号、换行、花括号、字面 `$`；缺值/非 UTF-8 | 不发生 TOML 注入/递归展开；错误不打印秘密 |
| CFG-04 | 默认样例、字段省略、边界范围/算术溢出 | 同一默认语义；无校验后改写 |
| CFG-05 | 热字段变化 / 相同文件 / 冷字段变化 / 冷热混合 | 只前者增代发布；其余按契约保持快照不变 |
| CFG-06 | generation 与值并发读取、无 receiver 发布、新订阅 | 代号/值一致；迟到读者能见到最新快照 |
| CFG-07 | 慢读者跳过中间发布、changed 与 borrow 竞争 | 允许跳代，最终收敛，不因长 borrow 阻塞 writer |
| CFG-08 | 慢重载、重复信号、超时后继续卡住 | 最多一个 blocking 作业加一个 pending 位；停止信号仍可观察 |
| CFG-09 | 关停同时完成重载、过期作业晚返回 | 不提交新配置，不恢复 Running |
| CFG-10 | writer 意外消失 / 正常关停消失 | 前者失败监督，后者正常退出，无 changed 忙循环 |
| TICK-01 | 启动门等待超过一个周期、周期更新、错过多个周期 | 观察提交后完整首周期；更新重置 deadline；Skip 不补发全部遗漏拍，测试考虑库的计时容差 |
| TICK-02 | tick、配置变化、取消同时就绪 | 取消优先，之后不发起新 tick；已发布与已消费代号不混淆 |

### 12.4 存储与 HTTP 验收

| ID | 场景 | 必须证明 |
| --- | --- | --- |
| DB-01 | 空业务迁移集首次启动 | 可连接/执行迁移器/自检；只有必要元数据，无模板业务表 |
| DB-02 | 测试 fixture 迁移成功与重跑 | 可重复启动，不重复应用已成功版本 |
| DB-03 | checksum mismatch、缺失版本、迁移 SQL 错误/dirty | fail-fast；不自动修复或忽略元数据 |
| DB-04 | 权限/打开失败、锁等待超时、自检失败 | 不提交 Running，池由外层所有者显式限时关闭 |
| DB-05 | 迁移文件新增、修改、删除后增量构建与跨平台 checkout | 新二进制看到正确嵌入集合；LF/校验和稳定；不靠 clean 掩盖缺少重编译跟踪 |
| DB-06 | 两实例竞争迁移、迁移中断后重启 | 锁等待有界，数据库状态重新验证，不声称中断必然完全回滚 |
| DB-07 | 生产/public 依赖面 | 只有 SQLite；API 公共签名和依赖中没有 sqlx；无绕过门面的连接出口 |
| HTTP-01 | 三个系统端点 | health 不访问 DB；ready 受 lifecycle+DB 限制；info 数据形状固定 |
| HTTP-02 | DB 挂起或失败、探测期间进入 Draining | 503 且有界；最后一次 lifecycle 检查不漏 |
| HTTP-03 | 所有前缀/根部未知路径、POST 到 GET | JSON 404/405，正确状态码；405 的 Allow、HEAD 的无 body 不被破坏 |
| HTTP-04 | 错误媒体类型、坏 JSON、错形状、超大 payload、坏 Path/Query | 保留 400/413/415/422；5xx rejection 泛化，不泄露输入 |
| HTTP-05 | handler future 超时、错误响应与 trace | JSON 503，不是空/纯文本；trace 不记录 query、header、body 秘密 |
| HTTP-06 | 慢请求/慢 header、强制停止、断开连接 | 有进程退出上限测试；不宣称所有情况都返回 JSON 或 graceful |
| HTTP-07 | 仅测试路由的 handler panic | 验证库内连接错误与顶层面退出的区别；不假定 supervisor 收到请求 panic |

### 12.5 生成工程验收与完成定义

| ID | 必须自动化的检查 |
| --- | --- |
| GEN-01 | 名称矩阵全部生成，crate 包名/导入名/二进制名/环境前缀正确，无未展开的模板变量 |
| GEN-02 | 区分 Liquid 残留与合法 GitHub 表达式、环境语法、Rust 转义花括号，不靠粗暴全局替换 |
| GEN-03 | 根据 `cargo metadata` 验 production/dev 依赖边与五 crate 清单，feature 闭包不带第二后端 |
| GEN-04 | 生成项目的 Makefile/CI 可用，无模板 gen/verify 目标引用，无 hooks / `.project` 垃圾 |
| GEN-05 | short/long/hyphen/撞词名称的真实 fmt、clippy、test 全绿；第二组 clean-room 全量构建 |
| GEN-06 | 同步、clean、overwrite 路径保护及并行生成测试；不修改模板以外的已有项目 |
| GEN-07 | 真实 release 二进制 smoke 与 panic/关停验收；生成服务并非只在 debug 正常 |

**可用模板的完成定义**：上述矩阵实施并跑绿、生成项目从零可复现、默认/extra 两条拓扑都经过真实进程验证、
已知关停限制写入生成项目 README、没有任何未确认 P0。不能以“示例能 curl”或“cargo test 无业务测试所以全绿”代替。

---

<a id="section-13"></a>

## 13. 分阶段实施计划

M0–M4 已于 2026-09-14 完成本阶段实现与本地验收。M5 产品化实现已落地，macOS arm64 本地最终验收已通过。
M1–M5 历史记录保留；维护阶段完成后，持续回归与平台范围统一记录在 `docs/verification.md`。
远端 CI 尚未触发，不能将本地通过等同于所有 runner/平台已通过或模板已发布。

| 阶段 | 目标、任务与交付 | 验收 / 主要风险 |
| --- | --- | --- |
| M0：设计评审（已完成） | 交付本文；单后端、冷热边界、force 语义、最小 crate 面均已确认 | 用户于 2026-09-14 接受全部四项 P0；不代表实现或技术验证完成 |
| M1：工程基线与高风险验证（已完成） | 五 crate 基线、固定工具链/依赖、生成器/Makefile/CI 分离、稳定内部别名与结构化锁文件映射 | macOS arm64 本地通过：默认/长名/依赖同名/clean-room 四组；14 个 Rust 测试、11 个 Python 工具测试及真实 release panic；完整记录见 M1 报告，远端 CI 尚未执行 |
| M2：单 runtime 垂直闭环（已完成） | 冷配置加载、StorageOwner、实际 TaskSupervisor、ticker、系统端点、启动提交、OS 信号、统一 deadline 和同步 runtime 销毁 | macOS arm64 本地通过 51 个 Rust 测试、debug/release 各 13 组真实进程检查、实际监督器的 release panic fixture；具体范围与未完成项见 M2 报告 |
| M3：热重载（已完成） | 类型化冷热边界、同一加载上下文、单飞 blocking 作业、整批拒绝/原子发布、ticker 重建周期、关停收割整合 | 本地通过 72 个 Rust 测试、debug/release 各 19 组实际进程检查；覆盖过期占槽、合并请求、迟到结果、loader panic、关停不发布和新周期连续两拍，详见 M3 报告 |
| M4：可选 runtime（已完成） | 完整拓扑校验、冷绑定、RuntimeSet/Executors、实际任务 runtime 元数据、部分构建失败清理、统一剩余期限与主 runtime 最后关闭 | 本地通过 82 个 Rust 测试；debug/release × 五种布局，每组合 23 组实际进程检查；另验证额外 worker 饱和时主 HTTP 响应、部分失败与全局关闭预算，见 M4 报告 |
| M5：模板产品化（实现与本地验收已完成） | 四组名称/格式/依赖矩阵、实际生成项目 Makefile、CI 分离/版本缓存键、空 target、迁移与协议负向证据、README/55 项映射 | GEN 自动门禁 + 真实 debug/release 五布局；平台/远端执行边界单独记录，不以静态 workflow 校验代替实际运行 |

各阶段只在真实消费者落地时建文件/依赖；不先生成完整空目录树再逐个填。
M2 可作为内部审阅的单 runtime 切片，不得宣称完整模板已验收，因为可选拓扑和生成链仍是本次目标的一部分。

对现有分支不做原地业务迁移，也不 cherry-pick 整个备份。
需要复用工程件时记录来源对象与改造差异；后续以小提交推进，任何失败都可以退回上一已验证阶段。
数据库迁移一旦被生成服务实际使用，回滚代码不自动回滚 schema，遵守 §8 的迁移纪律。

### 13.1 与 edge_dev api 层的刻意分叉

蓝本为 `edge_dev` 对象 `8ef2153` 的 `api/src/response.rs` 与 `api/src/error.rs`（见 §15.1）。
响应契约三条规则与 503/500 分界原文已整体搬入，下列差异是有意保留的，不是尚未对齐：

| 分叉 | 模板的选择 | 理由 |
| --- | --- | --- |
| `HttpError` 的描述类型 | 保持 `&'static str`，不放开为 `String` | edge_dev 的六个业务变体各自带 `String`，是业务路由要指名资源的结果。模板没有业务路由，收紧成 `&'static str` 后「5xx 不含用户输入」由编译器保证；首个业务变体自带 `String` 并只在 4xx 渲染 |
| 业务错误变体与 406 校验契约 | 不预置 `BadRequest`/`ValidationError`/`Conflict` 等变体 | §9.3 已声明业务错误码属于首个真实 API 的契约设计；空变体只会被复制粘贴成错误的语义 |
| `From<StorageError> for HttpError` | 不提供 | 模板的 `StorageError` 只带 `phase()`（connect/migrate/health），任何映射都只能产出 500，写出来是零信息的仪式 |
| 404 响应体 | 不回显 method 与 path | 比 edge_dev 更严：未认证调用方只应知道「没有匹配」。`Allow` 头仍由 axum 提供，那是客户端真正要用的部分 |
| `Rejected(StatusCode)` 变体 | 模板独有 | 模板的 extractor 包装要保留框架自己判定的 4xx（415/400/422/413），edge_dev 在业务层没有这条路径 |
| `thiserror::Error` 派生 | 只派生 `Debug` | edge_dev 需要它是因为 `#[from]` 传播；模板的 `HttpError` 只被转成响应，派生 `Display` 会让文案有两处来源 |

---

<a id="section-14"></a>

## 14. 方案取舍、风险与决策记录

### 14.1 为什么不选另外几种方案

| 方案 | 本版不选的原因 | 何时重开讨论 |
| --- | --- | --- |
| 单 crate + 任意 `tokio::spawn` | 不满足模块依赖纪律，任务寿命只能靠人脑追踪 | 本次目标不变时不讨论 |
| 七/八 crate，保留旧 reconcile/testkit/events/metrics | 没有当前消费者，为未来想象支付维护成本 | 首个真实动态期望集或跨 crate 夹具出现 |
| core 放 supervisor、BootConfig、信号、全局错误 | core 成为装配层搬家，不再是最小共享契约 | 至少第二个独立宿主真正复用具体能力时重新评审 |
| wrapper watcher + JoinHandle 注册 | 多一层任务，必须双持 abort；不能保留面自己的结果就更容易漏错 | 外部库只能给 JoinHandle 且必须接入时，单独设计受控 adapter |
| 改 runtime 绑定即热迁移任务 | 涉及 I/O 所属、资源交接、重复实例与任务重启，已经超出模板 | 有明确在线迁移需求时作为独立设计 |
| 冷字段回滚，热字段顺带应用 | 操作者提交的一份配置变成部分真值，报告/回滚规则容易漂移 | 有明确部分提交需求且能解释跨字段不变量时 |
| SQLite + PG 编译双后端或生成时多选 | 增加配置、feature、迁移与 CI 矩阵，不符合本次单后端 | 用户明确扩展模板产品范围时 |
| 为 force 关停重写 HTTP accept/连接管理 | 引入大量连接所有权代码和额外依赖，偏离最小起点 | 必须在同一进程强关连接后继续运行时 |
| 本版添加硬退出 watchdog / 子进程计算框架 | 扩大平台和异常处理面；默认 ticker 不需要 | 真实任务含不可中断代码且有明确硬截止目标时 |

### 14.2 风险台账

| 等级 | 风险 | 缓解与验收 |
| --- | --- | --- |
| P0 | 实现偏离已确认的单后端基线，重新带入双后端 | SQLite 已确认；DB-07 / GEN-03 验证实际依赖与 feature 闭包，不只检查配置默认值 |
| P0 | 误把有界等待当作线程终止、把 HTTP 外层 abort 当排空 | §5.6/§9.5 明示失败边界；正常/forced 分开测；外部硬截止不内化成虚假保证 |
| P0 | `panic=abort`、第三方 spawn 或不可中断工作破坏监督承诺 | release fixture 与子进程故障注入；每个新子任务补所有权证据 |
| P0 | 启动取消丢失半初始化池，或启动确认前接流量 | 外层资源所有者、目标 runtime 确认、单点提交；LIFE-01–04 |
| P1 | sqlx/axum/Tokio 升级改变迁移/关闭行为 | 依赖锁定；官方资料不是兼容测试替代；DB/HTTP/RT 矩阵随升级重跑 |
| P1 | 重载文件阻塞占住线程、反复重试形成堆积 | 一个槽位、一个 pending 位、过期结果丢弃、已开始 blocking 作业不假称能 abort |
| P1 | 只测解析、不测真实生成，名字替换破坏依赖或 CI | GEN 名称矩阵、结构化锁文件更新、clean-room 和路径保护 |
| P1 | 额外 runtime 隔离效果被夸大 | 记录实际预算，不承诺 CPU 配额；加入主 runtime 响应性测试，不用空负载截图证明隔离 |
| P1 | DB 元数据不一致被“自动修复”破坏数据 | 无自动修复；启动 fail-fast，迁移与恢复操作单独审查 |
| P2 | 后续业务持续往 core/AppState 填字段 | 每字段消费者准入与 manifest 边检查；不以未来可能有用为理由 |

### 14.3 实施前确认结论（P0，已全部接受）

**确认日期：2026-09-14。确认人：用户。** 用户明确回复：“4条问题我均接受、按照你的方案即可”。
以下四项由推荐方案转为本次实施基线，不再保留互斥选项：

| 决策 | 已确认结论 | 实施约束 |
| --- | --- | --- |
| P0-01：后端选择 | **SQLite 单后端** | 一套驱动、一套迁移、一个池；不带 PostgreSQL 实现、后端选择器或双后端 feature 矩阵 |
| P0-02：关停承诺 | **协作路径证明排空；forced 路径非零退出，允许丢失在途工作** | 不把 abort 或 runtime 等待返回当成线程/连接全部终止的证明；不可中断代码的硬截止依赖外部强制终止 |
| P0-03：热重载范围 | **仅 `ticker.interval_ms` 可热改** | 冷字段变化、冷热混合变化均整批拒绝发布，需重启；runtime、绑定、监听、池参数和关闭预算不热改 |
| P0-04：最小骨架 | **五 crate，release 保持 unwind** | `app / api / worker / storage / core`；不预置 reconcile、testkit、EventBus、Metrics |

**决策确认不等于技术风险已消失，也不等于实现验收已通过。** §14.2 的技术风险仍须通过 §12 的测试验证；
§14.4 的依赖兼容性、预算默认值和平台支持范围在实施过程中确定。若新证据要求改变上述已确认契约，
应提出具体变更及影响，不得静默扩展范围或回到原有待选状态。

### 14.4 实施中确认（P1）

- **M1 已完成**：固定并验证 Rust/MSRV、Tokio、axum、sqlx、cargo-generate 基线与当前所需 feature；未照抄旧锁文件。后续新增能力仍须评审新增 feature。
- 用真实负载/最慢测试确定各 deadline 的默认数值和资源上限；这些是可改模板策略，不是部署 SLA。
- **M1/M2 已验证**：空迁移、多个连接的外键选项、迁移目录增改删重编译、初始化取消后 owner 可关闭、未知/失败迁移元数据拒绝启动、真实进程在数据库初始化中响应停止。**M5 已补充**测试专用真实 schema 的版本重跑、checksum/missing/dirty/SQL 错误、独占锁超时、双初始化者竞争及 SQLite progress-handler 中断后重启。任意业务迁移、非事务 SQL 和断电恢复仍不在这些证据的承诺内。
- 首次已验证平台为 macOS arm64；Linux/macOS CI 定义已提供，但远端 Actions 未运行，不宣称 Linux/Windows 已验收。扩展支持范围必须补对应信号、PTY、真实进程与 checkout 证据。
- 若 M1 发现 axum 强制路径不能满足已批准 P0，回到 HTTP 所有权设计评审，不能默默升级承诺。

### 14.5 不阻塞本次的后续问题（P2）

首个业务仓储的事务边界、认证/CORS/TLS、部署端口和目录、metrics、计算池、动态 reconcile、
PostgreSQL 替换或另一生成模板，都由真实项目需求触发。本版不给这些问题预留空结构或占位实现。

---

<a id="section-15"></a>

## 15. 证据索引与官方语义依据

### 15.1 本地代码证据

所有初始取证于 2026-09-14；参考项目与备份对象只读，没有修改 `edge_dev` 或将备份整体检出到当前工作树。
本仓库的 M1 增量实现另见 §15.3 和验证报告。

**源项目**，对象 `8ef215323facace7f6fb5caa6ddfc662c15f508a`：

- `/Users/riotian/Documents/code/quasar/prism/edge_dev/Cargo.toml`
- `/Users/riotian/Documents/code/quasar/prism/edge_dev/app/src/boot.rs`
- `/Users/riotian/Documents/code/quasar/prism/edge_dev/core/src/task/supervisor.rs`
- `/Users/riotian/Documents/code/quasar/prism/edge_dev/core/src/config/app.rs`
- `/Users/riotian/Documents/code/quasar/prism/edge_dev/core/src/config/store.rs`
- `/Users/riotian/Documents/code/quasar/prism/edge_dev/storage/src/lib.rs`

**备份对象**，`1ac758474aa09c0e5f31ef93547db5a9e7c5fcbb`。
以下是该对象内的历史路径，当前工作树中并不必然存在同名文件；通过 `git show <对象>:<仓库相对路径>` 读取：

- `/Users/riotian/Documents/code/axum-starter-template/docs/architecture.md`（历史版本，不是本文）
- `/Users/riotian/Documents/code/axum-starter-template/core/src/task/supervisor.rs`
- `/Users/riotian/Documents/code/axum-starter-template/core/src/config/store.rs`
- `/Users/riotian/Documents/code/axum-starter-template/app/src/main.rs`
- `/Users/riotian/Documents/code/axum-starter-template/app/src/rt.rs`
- `/Users/riotian/Documents/code/axum-starter-template/app/src/signals.rs`
- `/Users/riotian/Documents/code/axum-starter-template/api/src/lib.rs`
- `/Users/riotian/Documents/code/axum-starter-template/api/src/response.rs`
- `/Users/riotian/Documents/code/axum-starter-template/storage/src/backend/mod.rs`
- `/Users/riotian/Documents/code/axum-starter-template/storage/build.rs`
- `/Users/riotian/Documents/code/axum-starter-template/Cargo.toml`
- `/Users/riotian/Documents/code/axum-starter-template/cargo-generate.toml`
- `/Users/riotian/Documents/code/axum-starter-template/hooks/pre.rhai`
- `/Users/riotian/Documents/code/axum-starter-template/hooks/post.rhai`
- `/Users/riotian/Documents/code/axum-starter-template/Makefile`
- `/Users/riotian/Documents/code/axum-starter-template/Makefile.project`
- `/Users/riotian/Documents/code/axum-starter-template/scripts/template-sync.py`
- `/Users/riotian/Documents/code/axum-starter-template/scripts/check-fmt-portability.py`
- `/Users/riotian/Documents/code/axum-starter-template/.github/workflows/ci.yml`

这些证据用于解释为什么保留或改写一项纪律，不代表已证明旧实现所有其他路径都正确。

### 15.2 官方语义依据

下列资料用于约束设计中的能力声明。检索日期为 2026-09-14，`latest` 页面是资料入口，
**不是实施依赖版本**；M1 已将实际采用的版本固定在 manifest/工具链/锁文件，并在验证报告中记录结果，升级时必须重新验证。

| 标记 | 一手资料 | 本文使用的语义 |
| --- | --- | --- |
| R1 | Tokio `JoinSet` | 直接 spawn_on；join 的结果/任务 id；abort 后仍需收割；Drop 请求 abort |
| R2 | Tokio `Runtime` | runtime 所有权、同步 shutdown_timeout、超时不杀死仍在执行的阻塞工作 |
| R3 | Tokio `spawn_blocking` | 开始执行后不能靠 abort 终止；并发上限和关闭等待必须单独考虑 |
| R4 | SQLx `Pool`、`migrate!` | 显式 close 的意义；迁移嵌入与 build.rs 目录重编译跟踪 |
| R5 | Tokio `watch` | 最新值、send_replace、borrow_and_update、借用锁及并发注意事项 |
| R6 | axum `serve`、Router、extract rejection | graceful、fallback/method fallback、提取失败状态与网络错误边界 |
| R7 | axum `serve` 实现源码 | 库内部连接任务及 graceful 等待；外层 future abort 不是递归收割证明 |
| R8 | Cargo profiles | unwind 与 abort 对 panic 处理的区别 |
| R9 | Rust Reference：dyn compatibility | trait object 的方法约束；不能将 native async trait 方法直接假定为 dyn-compatible |
| R10 | Tokio-util `CancellationToken` | child 与 clone 的取消权限区别、取消并非原子操作、DropGuard |
| R11 | Tokio `MissedTickBehavior` | Skip 跳过错失时刻，不逐次补发历史 tick |

资料地址（按标记分组）：

```text
R1  https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html
R2  https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html
R3  https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html
R4  https://docs.rs/sqlx/latest/sqlx/struct.Pool.html
    https://docs.rs/sqlx/latest/sqlx/macro.migrate.html
R5  https://docs.rs/tokio/latest/tokio/sync/watch/index.html
    https://docs.rs/tokio/latest/tokio/sync/watch/struct.Sender.html
R6  https://docs.rs/axum/latest/axum/fn.serve.html
    https://docs.rs/axum/latest/axum/struct.Router.html
    https://docs.rs/axum/latest/axum/extract/rejection/index.html
R7  https://docs.rs/axum/latest/src/axum/serve/mod.rs.html
R8  https://doc.rust-lang.org/cargo/reference/profiles.html
R9  https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility
R10 https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html
R11 https://docs.rs/tokio/latest/tokio/time/enum.MissedTickBehavior.html
```

### 15.3 本次交付边界

截至 v0.7，M0–M5 已实现，M5 本地产品化门禁通过。默认单 runtime 和可选 extra 绑定均保留；同一监督器管理实际任务，正常路径先关池、逆序关 extra、最后关主 runtime，所有 runtime 沿用同一剩余期限。
当前证据见 `docs/verification.md` 与 `docs/acceptance.md`，此前报告保留历史语境；生产迁移目录仍无业务 SQL，`edge_dev` 未修改，备份未整体恢复。
仅 ticker.interval_ms 可热改，runtime/绑定仍为冷配置。M5 产品化与本地验收状态见专门报告；远端 CI、Linux/Windows、任意业务迁移和生产 SLA 不被本地结果自动覆盖。
若新证据要求改变已确认 P0，应提出具体变更并同步设计和验收项，不得静默扩大承诺。

### 15.4 修订记录

| 日期 | 版本 | 变更 |
| --- | --- | --- |
| 2026-09-14 | v0.1 | 初始重新设计；完成本地取证、职责边界、生命周期、配置/存储/HTTP 契约与待实施验收矩阵 |
| 2026-09-14 | v0.2 | 记录用户接受全部四项 P0，确定 SQLite 等实施基线，M0 标记完成；补充 config 装配与共享契约的划分依据；未开始代码实施 |
| 2026-09-14 | v0.3 | 完成 M1 工程与本地验证；固定版本、稳定内部依赖别名、结构化锁文件映射与实际构建树后端门禁；记录四组生成/编译、release panic、HTTP force 和迁移重编译证据；M2 尚未开始 |
| 2026-09-14 | v0.4 | 完成 M2：冷配置、实例化生命周期、实际监督器、启动提交、系统 HTTP 契约与有界关停；增加 debug/release 真实服务进程验收与 M2 报告；M3/M4/M5 未实施 |
| 2026-09-14 | v0.5 | 完成 M3：冷热类型拆分、单飞加载、原子 watch 快照、ticker 周期重建和共享期限收割；验证超时占槽/合并/迟到结果/panic/关停竞态；M4/M5 未实施 |
| 2026-09-14 | v0.6 | 完成 M4：可选冷拓扑/绑定、同步 RuntimeSet 所有权、实际 runtime 任务标签、部分构建失败清理及统一关闭期限；验证五布局×双 profile，并修复本地 Python 缓存的生成过滤；M5 未实施 |
| 2026-09-14 | v0.7 | M5 产品化：名称/格式/依赖与并行生成矩阵、父 workspace 保护、生成项目真实 gates、CI 缓存/版本固定、迁移与协议负向证据、配置来源和 55 项映射；最终本地结果与未验平台分别记录在 M5 报告 |
| 2026-09-14 | v0.8 | 生成物去除实施阶段与未交付设计引用；独立 release 探针归入 tests/fixtures，保留真实 profile 回归；新增 plain gen 输出审计、用途化缓存/报告与旧目录显式清理兼容，持续结果改记 verification.md |
