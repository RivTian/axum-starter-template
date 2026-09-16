# 从零设计 axum-starter-template

> 一份可复用的 Rust Web 服务 cargo-generate 模板。本文是任务书，不是设计文档——设计文档由你产出。

---

## 0. 任务一句话

在 `axum-starter-template` 仓库里**从零设计并实现**一套 Rust Web 服务模板：workspace 多 crate 单向分层、
多线程 Tokio runtime + `TaskSupervisor` 顶层任务面（含可选的多 runtime 绑定）、根 `CancellationToken`
级联关停、三段式配置 + 热重载、`Arc<dyn Storage>` 存储门面。仓库里已有两次失败尝试的备份分支，
它们是**取证材料**，不是可以挑一份改出来的基线。

---

## 1. 仓库事实（已核实，不必重新确认）

| 事实 | 内容 |
| --- | --- |
| 远端 | `https://github.com/RivTian/axum-starter-template.git` |
| `main` | 只有一个 commit `71b5540 chore: initialize repository`，工作树**空** |
| 备份分支 A | `origin/backup/main-20260913-multi-rt-design` |
| 备份分支 B | `origin/backup/main-20260915-multi-rt-design` |
| 血缘 | A 与 B **无共同祖先**（`git merge-base` 为空），是两次彼此独立的重写 |
| 审计结论 | 两份都经同事复审，细节缺陷多到无法投产。**没有可增量修补的基线** |

分支 A 与 B 的形态差异（供你定位取证，不是让你二选一）：

| 维度 | A（0913） | B（0915） |
| --- | --- | --- |
| workspace 成员 | 7 个：`api app core reconcile storage testkit worker` | 5 个：`app api core storage worker` |
| 存储 | `Arc<dyn Storage>` + SQLite/PostgreSQL 双后端 + 成对迁移 | 单后端最小编译闭包（只编译 SQLite） |
| Rust 源与 Liquid | `.rs` 经 Liquid 渲染，含 `{{crate_prefix_snake}}` 等占位符 | `.rs` 逐字节复制；只渲染 `Cargo.toml` / `Cargo.lock` / `README.project.md` |
| 改名手法 | 模板源码里写占位符 | 根 manifest 用 `service-core = { package = "{{crate_prefix}}-core", path = "core" }` 别名 |
| 模板仓库门禁 | 5 条：`fmt-portable` / `fmt-check` / `fmt-matrix` / `lint` / `test` | `tooling-test` + `check` + `matrix` + `verify`，驱动器是 `scripts/template.py` |
| release profile | `panic = "abort"` | `panic = "unwind"`（注释：abort 会绕过 supervisor 观察 task panic 与 `CatchPanicLayer`） |
| 依赖钉法 | 语义版本（`"1"`、`"0.8.9"`） | 全部精确钉（`=1.53.1` 等）+ `rust-toolchain.toml` 钉 `1.97.1` |
| 测试布局 | `tests/` 集成目录 + `testkit` 夹具 crate | 测试与源码同 crate（`api/src/tests.rs` 等）+ Python 真实进程探针 |
| 验收 | 纪律清单 → 落点（文件级） | 55 条验收 ID → 可重复证据 + **限制声明**（`docs/acceptance.md`） |

---

## 2. 输入材料与读法

1. **`origin/backup/main-20260913-multi-rt-design`**
   - 必读：`docs/architecture.md`（§1 目标/非目标、§2 纪律清单、§3 crate 划分与依赖矩阵、
     §4 runtime 拓扑、§6 横切能力清单、§7 部署布局）、`cargo-generate.toml`、`Makefile`、
     `Makefile.project`、`hooks/*.rhai`、`scripts/*.py`、`.github/workflows/ci.yml`。
   - 读法：这是本次**分层划分与工程链的形态基线**。它的「每条纪律配一条理由、每条理由配一个落点」
     的写法要继承。
2. **`origin/backup/main-20260915-multi-rt-design`**
   - 必读：`docs/architecture.md`（尤其 §5 任务监督与进程生命周期、§7 配置与热重载、§9 HTTP 契约
     与就绪语义、§12 测试策略与验收矩阵、§14 风险台账）、`docs/acceptance.md`、`cargo-generate.toml`、
     `Cargo.toml`、`scripts/template.py`。
   - 读法：这是本次**验收严格度与若干关键语义修正的取证来源**。它比 A 晚两天，很多地方是在修 A 的洞
     （启动提交门、panic 策略、Liquid 渲染面收窄）。
3. **参考实现 `~/Documents/code/quasar/prism/edge_dev`**（模板骨架层的原始蓝本）
   - 读法：只取证「生产里这条纪律长什么样」，**不照抄**。B 的架构文档 §13.1 明确记录了与它 api 层的
     刻意分叉，先读那一节再决定要不要看。

**取证纪律**：凡是引用某个仓库的事实，写清「文件路径 + 结论」；引用 tokio / axum / sqlx 的语义，
写清版本号与出处（源码路径或官方文档锚点）。不写行号——行号会烂。

---

## 3. 「从零设计」的定义

**是**：

- 重新推导每一个决策，能说出为什么不选另一种；
- 可以采纳 A 或 B 的某个结论，但必须在设计文档里说明「采纳的是结论，理由是 X，原实现的问题是 Y」；
- 目录、crate 划分、trait 形状、配置分段、测试布局都重新定。

**不是**：

- checkout 某个分支然后打补丁；
- 把 A 的 `docs/architecture.md` 改几段当成新设计；
- 把 B 的 55 条验收 ID 原样搬过来当验收标准。

---

## 4. 已定决策（不再论证，直接执行）

| # | 决策 | 说明 |
| --- | --- | --- |
| F1 | **分层与 crate 划分以 A（0913）为形态基线** | 层次感与职责边界已被确认满意。调整某条边必须给理由，不是默认自由 |
| F2 | **工程链沿用 A 的形态** | `cargo-generate.toml` + `hooks/pre.rhai` / `post.rhai` + 模板 `Makefile` / 生成结果 `Makefile.project` + `scripts/` 维护脚本 + 模板自己的 CI。**形态**沿用，**实现**允许重写（见 DP1） |
| F3 | **存储走单后端最小编译闭包（B 路线）** | 只编译一个后端（SQLite）。但门面纪律不因此松动，见 §5.7 |
| F4 | **模板语言与注释语言：中文** | 设计文档、README、代码注释全中文；标识符、日志 message、测试名用英文 |
| F5 | **生成结果不携带 CI 配置** | 新项目的 CI 平台不定。门禁的唯一定义是生成结果的 `make check`。模板仓库自己保留一份 workflow |
| F6 | **不带任何具体部署事实** | 交叉编译目标、容器链、glibc 门禁、写死端口都不进模板 |

---

## 5. 必须成立的架构主题

每一条给出**判据**——设计文档里必须能指到一处落点，实现里必须有一条自动化证据。

### 5.1 workspace 多 crate 单向分层

- 依赖单向、叶子稳定；`core` 不依赖任何兄弟；邻接表之外的边一律非法，新增边写理由。
- 第三方版本与 feature 只在根 `[workspace.dependencies]` 声明一次，成员一律 `{ workspace = true }`；
  **每个第三方 crate 旁一句「为什么」**，没理由的依赖不该在表里。
- 窄门面：`lib.rs` / `mod.rs` 只做 `mod` 声明与显式 `pub use`，不做扁平化 re-export，每个例外出口写理由。
- **判据**：邻接表能在门禁里被机械校验（依赖图检查），不是只写在文档里。

### 5.2 多线程 Tokio runtime + `TaskSupervisor` 顶层任务面

- 顶层任务经 supervisor 注册；first-failure：任一顶层任务退出即进程退出。
- 任务面**返回 future**，由装配层决定 spawn 到哪；面内不写死 `tokio::spawn`。
- 收敛模式固定且写进文档注释：批量 cancel → 限时收割 → 超时 abort（任务组）；`select!` 上取消是
  第一分支（循环体）；`Drop` 兜底 abort（长活结构）。
- 退出分类必须区分：正常返回 / 返回错误 / panic / 被外部 abort，且分类携带任务名。
- 空 supervisor 正常退出，不判 first-failure。
- **判据**：「不留 detached task」「不留同 key 双化身」各有一条测试。

### 5.3 可选的多 runtime 绑定

- 默认路径是**一个**多线程 runtime；附加 runtime 由配置 `[runtime.extra.<name>]` 声明，缺省零子表 =
  零额外线程、零额外依赖、零面代码改动。
- 绑定写在**每个面自己的配置段**（如 `[ticker].runtime`），不是集中一张表。
- `resolve(None)` → 主 runtime；`resolve(Some(未配置的名字))` → **启动错误**，不静默回落
  （回落会把「配置写错」藏成「性能不达预期」）。
- 准入清单：只有 CPU 密集 async 循环、阻塞型 FFI/同步 SDK 打满 blocking 池、延迟敏感面需要隔离
  这三种场景值得单开，且**先量出来再开**。I/O 密集的面一律不值得。
- **判据**：设计文档要有一张「不用多 runtime 的服务为它付出了什么」的逐项代价表，每项能被验证。

### 5.4 根 `CancellationToken` 级联 + 限时收割

- 一切任务用 `child_token()`；子令牌不能取消父或兄弟。
- 关停序列固定：广播关停事件 → cancel → 限时收割顶层任务 → **存储最后关** → 退出 `block_on` →
  同步关闭 runtime（附加逆序、主最后）。
- 关停预算分层：内层收割预算 < 外层宽限，留余量；第二次停止请求**加速**而不刷新截止时间。
- 多次关停、超时未收割的任务，都要有诚实的报告（不假报 graceful）。
- **判据**：每一段预算都有上限且有测试钉住；「关池超时不替换原始失败」有测试。

### 5.5 三段式配置加载 + `watch` 热重载

- 一条管线，启动与热重载**共用**：读文件（缺文件落盘内嵌模板）→ 环境变量展开 → 反序列化（带字段
  路径定位）→ 路径解析 → 补全 → 校验 → 钳位。两条管线迟早漂移。
- 校验只拦「继续跑必然失败」的取值；其余越界钳位 + warn。无人值守下拒绝启动的代价远大于按保守值跑。
- 每段 `#[serde(default, deny_unknown_fields)]`：拼错键即错误，缺段等价默认，零配置可起。
- 写端只在装配层；读端句柄廉价 Clone 随处注入；可热字段每 tick 现读。
- 热度三档：可热 / 半热（启动闸）/ 不可热。不可热段 reload 时**回滚为运行值**并列入待重启清单——
  watch 里的配置必须永远等于「生效中」的配置。
- 重载失败保留 last-good，绝不半套用。
- 环境变量占位符定界符**用 `${NAME:default}` 形态，不用 `{{ }}`**（后者是 Liquid 语法，见 §6.4）。
- 内嵌模板取值与 `Default` 逐字段一致，有测试钉住。
- 敏感信息取值优先级固定（`*_env > *_file > 明文`），`Debug` 手写渲染为 `***`。
- **判据**：冷段判别方式不能是「按面登记」——漏登记时是**静默**失败（热重载假报已生效）。必须给出
  漏登记时会红的机制。

### 5.6 tracing 只在装配层初始化

- 库 crate 只 `use tracing` 发事件，永不装 subscriber。
- 日志形态**只出一种**，不留格式开关。
- 测试不装全局 subscriber；日志断言用私有 subscriber 捕获。
- 日志里不出现 query / header / body 的秘密。
- **判据**：有一条门禁能发现「某个库 crate 里出现了 subscriber 初始化」。

### 5.7 `Arc<dyn Storage>` 门面 + 迁移（单后端闭包）

已定 F3：只编译一个后端。**以下纪律不因单后端而放宽**：

- 公共 API 不出现 `sqlx` 类型；上层拿不到连接池。拿不到池就不可能绕过门面发 SQL。
- 唯一入口：连接 → 迁移 → 自检 → 返回，任一步失败 fail-fast；自检失败先显式关池再上抛。
- 存储层不持有常驻任务；维护动作由上层调度。
- 迁移只增不改、已发布不可动；`build.rs` 对迁移目录 `rerun-if-changed` **不可删**
  （`sqlx::migrate!` 对新增文件静默不生效）。
- 迁移元数据异常（校验和不匹配 / 版本缺失 / dirty）一律 fail-fast，**不自动修复元数据**，并给出
  可执行的修复指引。
- 错误按语义归一化（`NotFound` / `Conflict` / `UniqueViolation` / `VersionConflict`），不按表拆。
- `Storage: Debug`，只输出后端名，绝不打印连接串或凭据。
- 不预建表；不提供假成功实现；「怎么加一个仓储」由 README 的垂直切片步骤 + bootstrap 测试承载。
- **单后端带来的两处必须显式处理**：
  1. A 的「绝不单后端落地」纪律失去示范载体 → 设计文档必须重写这条纪律，说明换后端时靠什么保证
     门面不泄漏（例如：编译闭包检查 + 门面公共 API 的源码审查门禁）。
  2. 编译闭包必须可验证 → 要有门禁证明「二进制里只有一个后端」，且不把 `Cargo.lock` 里的可选解析包
     误判为已编译后端。

### 5.8 进程内零业务全局态

- 指标、事件总线、配置句柄、存储全部由装配层构造并注入，无 `static`/`lazy_static` 业务态。
- **判据**：一个进程里能同时跑两套独立装配，生命周期、配置、数据库、取消互不干扰，有测试钉住。
  这也是去掉 `--test-threads=1` 的前提。

### 5.9 部署布局与路径锚点

- 安装根 = **可执行文件所在目录**，永远不取 cwd。
- 配置文件位置可由 CLI / 环境变量覆盖，但覆盖**只改配置文件位置**，不改路径锚点。
- 配置里的相对路径（含 `data/`）以同一个锚点派生，不以「配置文件所在目录」为基准。
- 配置与数据分成两个同级目录（运维改 / 进程写，权限与备份策略不同）。
- **判据**：「换个目录启动读到另一份配置」这类故障要被结构性排除，有测试。

---

## 6. 模板仓库特有的坑（已付过学费，不要再踩）

这些是**模板仓库独有**的缺陷类，普通仓库里不存在。设计文档必须逐条声明「本设计怎么处理」。

1. **模板仓库自身不可直接 `cargo build`**（如果源码里有占位符）。所有 cargo 命令必须在「生成目录」
   里跑，生成目录放在仓库树**外**。
2. **rustfmt 不是名字无关的**。rustfmt 的排版取决于**替换之后**的行宽与排序，「对 `tpl/tplx` 这组
   名字 fmt-clean」不蕴含「对任意名字都是」。A 用 `fmt-portable` 静态扫描 + `fmt-matrix` 换名重跑
   来兜；B 用「`.rs` 不经 Liquid + `package` 别名」结构性消除。见 DP1。
3. **`ignore` 在 post hook 之前生效**。`hooks/` 不能进 `ignore`（脚本会先于自己被删掉），只能在
   post hook 里自删。
4. **Liquid 的 `{{` 与 Rust 格式串的转义花括号冲突**。凡是 `.rs` 走 Liquid 渲染，含 `{{` 的文件
   必须进 `exclude`，且这个名单随文件增减要维护。配置占位符也因此不能用 `{{ }}` 定界。
5. **生成结果不带模板仓库自己的 workflow**。模板的 CI 跑的是 `make gen` / `make verify`，生成结果的
   Makefile 里没有这些目标——照搬会给每个新项目一个开箱即红的 workflow。
6. **环境变量门控的测试，「静默跳过」与「全绿」长得一样**。任何「声明了却连不上」的外部依赖必须
   **红而不跳**；CI 里导出的环境变量前缀要与源码里的常量**对字面量**，不一致就红。
7. **`Runtime` 不能在 async 上下文里 drop**（tokio 直接 panic）。附加 runtime 只能在 `block_on`
   返回之后**同步**关闭；`shutdown_background` 不等任务收尾，顺序就看不出来了。
8. **附加 runtime 不能是 `current_thread`**。没有线程对它 `block_on`，spawn 进去的任务永不执行、
   `await` 其 `JoinHandle` 直接挂死。单 worker 的多线程 runtime 已给到同等隔离且自带驱动。
   残留的老配置字段要**当场报错**而不是静默挂死。
9. **tokio 的 IO 资源在创建时注册到当前 runtime 的驱动**。HTTP 面若绑到附加 runtime，socket 必须在
   那个 runtime 里完成注册（同步 bind + future 内 `from_std`）。共享资源（存储池、事件总线）一律在
   主 runtime 创建，主 runtime 最后关。
10. **`panic = "abort"` 会绕过 supervisor 观察 task panic**，也绕过 router 的 panic 捕获。A 用 abort，
    B 用 unwind 并写明理由。见 DP2。
11. **`Cargo.lock` 随模板发布**（`--locked` 门禁的前提）。锁文件里只有包名，反向映射要能处理。
12. **项目名进字符串字面量会让行宽随名字浮动**。用常量引用，不写字面量。
13. **生成结果里不得引用实施里程碑、阶段编号或未交付的设计文档**。注释就地解释当前的不变量、约束和
    失败边界。要有门禁在 `make gen` 阶段就审计输出，而不是等完整 `check` 才发现残留。

---

## 7. 必须在设计文档里逐条回答的决策点

每条给出：两个（或更多）方案 → 各自代价 → 你的选择 → 选择的判据。**不要合并、不要跳过**。
标 ⭐ 的三条是 A 与 B 的直接冲突，必须先解决。

| # | 决策点 | 冲突双方 |
| --- | --- | --- |
| ⭐ DP1 | Rust 源是否经 Liquid 渲染 | A：`.rs` 含占位符 + `fmt-portable`/`fmt-matrix`/`template-sync.py` 三件套兜底；B：`include` 白名单只渲染身份文件 + 根 manifest 用 `package` 别名，`.rs` 逐字节复制。**注意**：选 B 会让 A 的两个 Python 脚本与两条门禁失去存在理由，而 F2 说「工程链沿用 A 的形态」——形态指的是 `make gen` 出树外 / 五条门禁的骨架 / `cargo generate --test` 自检 / `lock` 回写，不是必须保留这两个脚本。明确论证后再定 |
| ⭐ DP2 | release `panic` 策略 | `abort`（体积与确定性）vs `unwind`（supervisor 能观察 task panic、router 能把 handler panic 答成统一错误信封）。与 5.2 的退出分类直接耦合 |
| ⭐ DP3 | 启动协议 | 「注册即运行」（A）vs 「准备不等于提交」的显式启动门（B：监听/注册 ≠ Running，门前不处理业务，缺确认有上限）。后者更严格但增加一层状态机 |
| DP4 | crate 集合 | `reconcile`（期望集/生命周期骨架，约 1800 行，默认无人依赖但随 workspace 一起测）与 `testkit`（dev-only 夹具）是否进骨架。F1 说以 A 为基线，但「默认不进二进制的可选 crate」要重新论证收益 |
| DP5 | 依赖版本钉法 | 语义版本（A）vs 全精确钉 `=x.y.z`（B）。模板发布 `Cargo.lock`，两者的实际差别是什么 |
| DP6 | 工具链钉法 | 只声明 MSRV 不钉通道（A）vs `rust-toolchain.toml` 钉死具体版本（B）。谁承担「升级 stable 就红」的成本 |
| DP7 | 测试布局 | `tests/` 集成目录 + `testkit` 夹具 crate（A）vs 测试与源码同 crate（B）。与 DP4 的 `testkit` 耦合 |
| DP8 | 冷段判别机制 | 后缀通判 + 冷前缀白名单（A，但 A 自己记了一个「回滚块仍按面硬编码，加面漏改静默」的已知缺口）vs 类型化冷段全等比较、冷热混改整批拒绝（B） |
| DP9 | 真实进程测试是否进门禁 | B 用 Python 驱动真实二进制跑数百个场景（信号、迁移失败、reload 竞态、release panic 夹具）。收益明确但维护成本高，且给模板引入 Python 依赖。取全量 / 取子集 / 不取 |
| DP10 | 存储启动的可取消性与关闭所有者 | 启动期间收到停止信号能否中断、谁负责关池、关池超时是否替换原始失败 |
| DP11 | HTTP 就绪语义 | `ready` 的判据是什么、探测超时如何有界、跨越 draining 的探测能否返回 ready |
| DP12 | 提取器包装是否预铺 | A 论证过「它补的不是新功能，是已写下契约上的洞」，并且状态码**不拍平**（400/413/415/422 各自保留）。与「不做预铺」的张力要重新裁决 |
| DP13 | 横切能力准入 | 逐项裁决：CORS / OpenAPI / Prometheus 导出 / HTTP 客户端 / 文件日志 / 配置 reload 端点。每条写「内置 / 可选 / 不进」+ 理由 |

---

## 8. 工作流程：四阶段、两道闸门

### 阶段 0 — 取证审计（无闸门，直接做）

- 通读 §2 的输入材料。
- 产出 `docs/audit-0913-0915.md`：**两份备份分支的缺陷台账**。
  - 每条格式：`ID | 分支 | 位置（文件路径） | 缺陷描述 | 缺陷类别 | 对新设计的约束`。
  - 缺陷类别至少分：语义错误 / 纪律无落点 / 纪律有落点但无证据 / 静默失败面 / 工程链缺陷 / 文档与实现不符。
  - **禁止用「细节没处理好」这类结论**。每条要能指到文件。
  - §7 的 13 个决策点，每个至少关联一条台账条目或显式说明「无既有证据，属新增决策」。

### 阶段 1 — 架构设计文档 → 🚧 **闸门 1：停下等人审批**

- 产出 `docs/architecture.md`。结构要求：

  ```text
  1. 目标 / 非目标 / 模板用户的「第一天」
  2. 纪律清单        每条：纪律 → 为什么 → 代码里的落点（文件级）→ 自动化证据
  3. crate 划分、分层图、依赖邻接表、明令禁止的边、每个 crate 的骨架内容
  4. runtime 拓扑     默认路径 / 多 runtime 绑定 / 跨 runtime 规则 / 关停顺序 / 零代价证明
  5. 进程生命周期     状态机、退出分类、启动协议、关停预算与顺序
  6. 配置与热重载     真值与热度定义、三段式管线、发布接口、重载事务
  7. 存储             门面、编译闭包、迁移纪律、关闭所有者
  8. HTTP 契约        最小状态、系统端点、响应与安全契约、就绪语义、graceful 边界
  9. 横切能力准入清单  内置 / 可选 / 不进，逐项理由
  10. 部署布局
  11. 模板生成与工程链  复用白名单、必须改造项、两类门禁
  12. 测试策略与验收矩阵  每条不变量 → 证据形态 → 已知限制
  13. 决策点裁决记录    §7 的 13 条逐条回答
  14. 风险台账与取舍记录
  15. 证据索引         本地代码证据 + 官方语义依据（带版本）
  ```

- 写作纪律：先摆事实再给方案；每条决策带理由；**每个「不选另一种」都要写清那一种的代价**。
- **闸门 1 的停止条件**：设计文档写完即停，输出一份「待拍板清单」（§7 里你给了选择但把握不足的、
  以及审计中新发现的分歧），等人确认后才进阶段 2。**不要自行开工**。

### 阶段 2 — 实现（按层推进）

- 顺序建议：`core` → `storage` → `worker` → `api` → `app` → 工程链（cargo-generate / Makefile /
  scripts / CI）→ README。
- **每一层落地后门禁必须全绿再进下一层**，不攒到最后。
- 每层落地时同步写 crate 级 README：边界 / 目录 / 关键决策 / 测试形态。
- 提交粒度按层切，commit message 说明这一层承担了哪几条纪律。

### 阶段 3 — 验收证据索引 → 🚧 **闸门 2：交付前汇报**

- 产出 `docs/acceptance.md`：设计文档 §2 的每条纪律 / §12 的每条不变量 → **可重复证据**（仓库相对
  路径 + 测试名或场景名）→ **结论与限制**。
- **不用测试总数替代逐项对应**。没有证据的纪律要显式标「无自动化证据」，不要留白。
- 产出 `docs/verification.md`：本次实际跑过的门禁、真实环境（OS / Rust / 工具版本）、结果。
  **没跑过的不写成跑过；远端 CI 与其他平台不因本地绿而视为通过。**

---

## 9. 交付物清单

```text
docs/audit-0913-0915.md        两份备份分支的缺陷台账（阶段 0）
docs/architecture.md           架构设计文档（阶段 1，闸门 1）
docs/acceptance.md             验收证据索引（阶段 3）
docs/verification.md           本次验证记录（阶段 3）
docs/references.md             外部取证：仓库 / 官方文档，带版本与出处

Cargo.toml                     workspace 根：成员、profile、依赖表（每条依赖带理由）
<各 crate>/                     src + README.md + 测试
cargo-generate.toml            placeholders / hooks / 渲染面白名单
hooks/pre.rhai                 派生变量与身份校验
hooks/post.rhai                Makefile.project / README.project.md 改名；hooks/ 自删
Makefile                       模板仓库的门禁（gen / check / verify / lock / clean）
Makefile.project               生成结果拿到的 Makefile（三条门禁）
scripts/                       模板维护工具（形态见 DP1 裁决）
.github/workflows/ci.yml       模板仓库自己的 CI（不进生成结果）
README.md                      怎么用与怎么维护这个模板
README.project.md              生成结果拿到的 README
```

---

## 10. 完成定义（DoD）

1. 模板仓库的 `make check` 全绿（门禁条目由 DP1 裁决后确定）。
2. `make verify`（`cargo generate --test`，**换一组名字、不复用 target、走 cargo-generate 自己的
   展开路径**）全绿。
3. 至少跑过一组**极端长名字**与一组**极短名字**的展开验证。
4. 生成结果在干净目录里：`make check` 三条门禁全绿；`cargo run` 能起、`curl` 系统端点得预期响应、
   Ctrl-C 后关停日志里每个任务都有收尾记录、退出码正确。
5. `docs/acceptance.md` 里每条纪律都有证据或显式的「无证据」标注，没有留白。
6. 生成结果里搜不到：实施里程碑词、阶段编号、未交付文档的引用、模板仓库自己的 Makefile 目标。
7. 新增的每一个第三方依赖，在根 manifest 里旁边有一句「为什么」。

---

## 11. 硬性禁止

- ❌ 把某个备份分支 checkout 出来改，或把它的设计文档改几段交付。
- ❌ 写「细节没处理好」「已优化」「更健壮」这类没有落点的结论。
- ❌ 声称跑过没跑过的门禁；声称验证过没验证过的平台。
- ❌ 预铺猜形状的东西：预建表、没有消费者的状态字段、只有一个文件的 `middleware/` 目录。
- ❌ 在库 crate 里初始化 tracing、建常驻任务、或让公共 API 出现驱动类型。
- ❌ 为「显得完整」而加依赖、加 feature、加配置项。
- ❌ 跳过闸门 1 直接开始实现。
- ❌ 子 agent 的使用遵守全局规范（`~/.claude/CLAUDE.md`）：默认不开，只在大范围只读检索、真正独立的
  多路任务、一次性长文本消化三种场景下开，且不递归派生。

---

## 12. 汇报格式

- 每个阶段结束时给一段**结论优先**的汇报：这一阶段定了什么、有什么没定、下一步做什么。
- 涉及文件时给 `路径:行号` 形式的引用，不贴大段原文。
- 遇到与本任务书冲突的事实（例如某条纪律在新方案下不成立），**先说出来再继续**，不要默默绕过。
- 闸门处必须停下等人确认，不要自答自批。
