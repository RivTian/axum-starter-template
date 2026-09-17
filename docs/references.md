# 第三方语义依据与实测记录

> 规则（任务书 §2 支撑约束·取证纪律）：引用第三方语义必须写清**版本号 + 出处**（源码路径或官方文档锚点），
> **不写行号**。本文件记录两类东西：① 本机在本机版本上**实测**到的机制（附命令与观察结果）；
> ② 来自源码/官方文档的语义依据。实测与任务书 §5 的表述不符处，以实测为准并显式标注。

## 0. 环境快照

| 项 | 值 |
| --- | --- |
| OS | macOS 26.6.2 (25G83), arm64 |
| rustc | 1.95.0 (59807616e 2026-04-14) |
| cargo | 1.95.0 (f2d3ce0bd 2026-03-21) |
| rustfmt | 1.9.0-stable (59807616e1 2026-04-14) |
| cargo-generate | 0.24.0（`cargo generate-generate 0.24.0`，`~/.cargo/bin/cargo-generate`） |
| 本地可用 toolchain | stable(1.95.0)、1.91.0、1.97.1 |
| crates.io 查询时间 | 2026-09-17（仅用于选版本；实现时以 `cargo tree` 的实际解析为准） |

## 1. 计划使用的第三方版本与理由

理由即根 `Cargo.toml [workspace.dependencies]` 的行内注释来源；此处是设计期冻结版本，阶段 1 编译后
用 `cargo tree --workspace --depth 1` 的实际结果回填/纠正本表。

| crate | 设计版本 | 为什么 |
| --- | --- | --- |
| `tokio` | 1.53 | 唯一异步运行时；supervisor 的 spawn/信号/时间/通道都建立在它上面 |
| `tokio-util` | 0.7 | `CancellationToken`：单化身取消与"重启=先取消旧化身"需要可克隆的协作取消原语 |
| `axum` | 0.8 | 模板名字的来源；`serve` + `with_graceful_shutdown` 是零路由也能成立的最小 HTTP 面 |
| `tracing` | 0.1 | 库 crate 只发事件、不装 subscriber；事件形态统一 |
| `tracing-subscriber` | 0.3 | 装配层唯一的 subscriber：fmt + `EnvFilter` + `reload`（可热日志级别） |
| `serde` | 1.0 | 配置反序列化与 diff；`deny_unknown_fields` 是"拼错键即错误"的实现手段 |
| `toml` | 1.x | 配置文件格式；带位置信息的错误 |
| `thiserror` | 2.0 | `core` 的错误分类；手写 `Display` 的样板在此规模不值得 |
| `sqlx` | 0.9 | 存储门面：异步 SQLite 池 + 迁移运行器；`sqlite` 特性默认 bundled，不需要系统库 |
| `notify` | 8.2 | 配置热重载的文件系统事件源（回调式暴露，不拥有 runtime） |
| `clap` | 4 | CLI 入口契约（`--config <path>` + `--help` + 未知参数拒绝）；只开 `derive` 特性。手写解析对一个参数够用，但用户加第二个参数时就会开始腐烂 |
| dev: `tempfile` | 3 | 测试用临时锚点/配置/数据库文件 |
| dev: `tokio`(`test-util`) | 1.53 | 暂停时钟钉住每段关停预算 |
| dev: `toml` | 1.x | `structure.rs` 解析成员清单做邻接表校验 |

未引入：`futures`（`Pin<Box<dyn Future>>` 与 `JoinSet` 已够用）、`anyhow`（错误分类要落盘到 `core`，
不用类型擦除）、`serde_json`（没有 JSON 面）、`thiserror` 之外的错误宏、任何 `once_cell`/`lazy_static`。

## 2. cargo-generate 0.24.0：源码依据

来源：本机 cargo 缓存 `~/.cargo/registry/src/index.crates.io-*/cargo-generate-0.24.0/src/`
（同版本官方文档站点 `https://cargo-generate.github.io/cargo-generate/`，页面如下标注）。

| 机制 | 依据 |
| --- | --- |
| 展开顺序：init hooks → 设定 `project-name`/`crate_name` → 填 placeholders → **pre hooks** → 按 `ignore` 删路径 → walk（渲染/拷贝）→ **post hooks** → 删 hook 文件 | `src/lib.rs` 的 `expand_template`；文档 `templates/scripting.hook-types.html` |
| `ignore` 是**字面相对路径**比对（`relative_path == entry`），不匹配通配符，且不存在时静默跳过 | `src/include_exclude.rs`（`Matcher::should_include` 的 `self.1` 比对）、`src/ignore_me.rs`（`remove_dir_files` 过滤 `exists`） |
| `include`/`exclude` 是 gitignore 语义的 glob（用 `ignore::gitignore::GitignoreBuilder` 构建），二者互斥（同时出现时 `include` 生效并 warn） | `src/include_exclude.rs` |
| `exclude` 只让文件**内容**不过 Liquid；文件名仍会做占位符替换；文件仍然会被拷贝 | `src/template.rs`（`ShouldInclude::Exclude` 分支里的 `substitute_filename` + `fs::copy`）；文档 `templates/include_exclude.html` |
| `x.liquid` → `x`（后缀在"拷贝进临时目录"阶段去掉），且**同名无后缀文件被遮蔽/跳过** | `src/copy.rs`（`LIQUID_SUFFIX`、`copy_file` 的两个 `exists()` 分支）、`src/lib.rs`（`strip_liquid_suffixes`）；文档 `templates/index.html` |
| Liquid 缺失变量 → 渲染成空串（`render_string_gracefully` 会插入空串重试）；Liquid **语法错误** → 收集后 `bail!`，生成失败并列文件名 | `src/template.rs` |
| hook 文件在 walk 时被当作 ignore（不渲染），在 post 之后才被删除；目录不删 | `src/template.rs`（`walk_dir` 的 `hook_files`）、`src/lib.rs`（post 后的 `remove_dir_files`） |
| rhai hook API：`abort(msg)`、`variable::{get,set,is_set}`、`file::{exists,rename,delete,write,listdir}`（沙箱在模板目录内）、`to_snake_case` 等大小写函数 | `src/hooks/mod.rs`（`abort` 与大小写函数注册）、`src/hooks/variable_mod.rs`、`src/hooks/file_mod.rs` |
| `crate_name` = `project-name` 输入的 snake_case；`project-name` = 已 snake 则保留、否则 kebab（`--force` 不改） | `src/template_variables/crate_name.rs`、`src/template_variables/project_name.rs` |
| 带 `CACHEDIR.TAG`（Cache Directory Tagging Spec 签名）的目录会被跳过；`.git` 跳过；符号链接跳过并告警 | `src/copy.rs`（`is_cache_dir`、`copy_files_recursively`）、`src/template.rs`（`filter_entry`） |
| 自动忽略：`.genignore`、`cargo-generate.toml`、`.cargo-ok`；`.gitignore` 不参与 | `src/ignore_me.rs` |
| `--define`/`--values-file`/环境变量提供的占位符值**也会**过 `[placeholders]` 的 `regex` 校验（不匹配报错） | `src/interactive.rs`（`handle_string_input` 的 provided 分支）、`src/project_variables.rs`（`MissingDefaultValueForPlaceholderVariable` 等） |
| abort 发生在渲染前，但**目标目录已创建**（草稿目录不会被清理） | `src/lib.rs`（`destination.create()` 在 pre hooks 之前） |

## 3. cargo-generate 0.24.0：本机实测（探针模板）

探针仓库（临时目录，未纳入模板仓库）：`cg-probe/tmpl`，含 `cargo-generate.toml`（`exclude = ["**/*.rs","Cargo.lock"]`、
`ignore = ["ignored.txt","subjunk","docs","no-such-path"]`、一个 `crate_prefix` 提示、pre/post hook）、
各类文件（`plain.rs`、`README.md`、`README-copy.md`、`Makefile.liquid`、`ignored.txt`、`subjunk/x.txt`、
`docs/y.md`、`Cargo.lock`、`.gitignore`、`.DS_Store`、嵌套 `crates/demo/Cargo.toml`/`notes.md`）。

命令（在生成目录外执行，`out*` 与仓库树分离）：

```sh
cargo generate --path ../tmpl --name probe-one --define crate_prefix=svc
cargo generate --path ../tmpl --name probe-two --define crate_prefix=waytoolongprefix   # pre hook abort
```

| 观察 | 结果 |
| --- | --- |
| `exclude` 命中的 `*.rs` | 内容保持 `pub const NAME: &str = "{{crate_prefix}}-core";`（未渲染） |
| `exclude` 命中的 `Cargo.lock` | 仍被拷贝到结果里（`exclude` ≠ 不生成） |
| `ignore` 命中的路径 | `ignored.txt`/`subjunk/`/`docs/` 都不在结果里；`no-such-path` 无报错（静默） |
| pre hook 写出的文件 | `pre-wrote.txt` 内容被渲染（`prefix=svc`）→ pre 在 walk 之前 |
| post hook 写出的文件 | `post-wrote.md` 内容是字面 `{{crate_prefix}}` → post 在 walk 之后 |
| pre hook 里 `file::exists("ignored.txt")` | `true` → `ignore` 删除发生在 pre 之后 |
| `Makefile.liquid` + 同名 `Makefile` 同时存在 | 结果里是 `.liquid` 的内容（无后缀文件被遮蔽）；`.gitignore.liquid` 同理 |
| `{{nope}}` | 渲染为空串，无报错 |
| 渲染面里的 `broken {{ liquid` | 生成**失败**：`Error: Substitution skipped, found invalid syntax in bad.md`，退出码 1（不静默） |
| pre hook `abort(...)` | 生成失败，退出码 1，**留下空的 `<destination>/probe-two/` 目录** |
| `.DS_Store` | 被拷贝进结果（`ignore` 只能按字面路径挡，挡不住子目录里的残留） |
| rustfmt 名字无关性 | `use q…_runtime::{ExitRecord, RuntimeId, TaskContext, TaskKey, TaskSpec};`（前缀 32 字符）被 rustfmt 重排为多行；单条目 `use`（含超 100 列）不被改写；超长字符串字面量所在行不被改写 |
| 成员包名与依赖图撞名 | 工作区成员 `axum-core 0.1.0` + 依赖 `axum 0.8`：`cargo metadata` 正常解析，`Cargo.lock` 里两条同名字段用 `version`（+`source`）消歧。**只有**"发布的锁 + `--locked`"才会失配 |
| cargo 包名规则 | 实测拒绝：数字开头（`1svc-core`）、`.`；接受：`svc-core`、`svc_x-core`、`svc--core`、`SVC-core`、`svc_core`、`a-core`、`x-core` |

与任务书 §5 的差异（以实测为准，已写进 `docs/architecture.md §12` 附录）：

- 雷区 11：0.24.0 会跳过带 `CACHEDIR.TAG` 的目录（cargo 的 `target/` 因此不会泄漏）；`.DS_Store` 等仍会泄漏。
  审计仍按"可能泄漏"做（不依赖单一版本行为）。
- 雷区 6：渲染面里的 Liquid 语法错误在 0.24.0 是**生成失败**（列文件名 + 退出码 1），不是静默原样拷贝。

## 4. tokio 1.53：语义依据

| 语义 | 出处 |
| --- | --- |
| `Runtime::shutdown_timeout(duration)`：等待至多 `duration` 收尾（阻塞任务池），`shutdown_background()` 不等任何 spawned work | `tokio-1.53.1/src/runtime/runtime.rs` 的相应方法文档；文档锚点 `https://docs.rs/tokio/1.53.1/tokio/runtime/struct.Runtime.html#method.shutdown_timeout`、`#method.shutdown_background` |
| 在 async 上下文里 drop `Runtime` 会 panic（阻塞不允许的上下文） | `tokio-1.53.1/src/runtime/blocking/shutdown.rs` 的错误串 `Cannot drop a runtime in a context where blocking is not allowed.`；文档锚点 `Runtime#shutdown` |
| `current_thread` runtime 的任务只有在被 `block_on` 驱动时才执行（因此"附加 runtime"必须用 multi-thread，或由拥有者持续驱动） | `tokio-1.53.1/src/runtime/runtime.rs` 的 `Runtime` 文档（runtime 种类与驱动说明）、`https://docs.rs/tokio/1.53.1/tokio/runtime/struct.Runtime.html#method.new_current_thread` |
| `JoinError::is_panic` / `is_cancelled` / `into_panic`：任务 panic 与 abort 的观察入口 | `tokio-1.53.1/src/runtime/task/error.rs`；文档锚点 `https://docs.rs/tokio/1.53.1/tokio/task/struct.JoinError.html` |
| `tokio::time::pause/advance`（`test-util`）：暂停时钟，用于把预算测试做成确定性 | `https://docs.rs/tokio/1.53.1/tokio/time/fn.pause.html` |
| `tokio::signal::ctrl_c()`；Unix 下 `signal::unix::{signal, SignalKind}` 收 SIGTERM | `https://docs.rs/tokio/1.53.1/tokio/signal/fn.ctrl_c.html`、`https://docs.rs/tokio/1.53.1/tokio/signal/unix/index.html` |
| IO 资源在其创建所在的 runtime 上注册；跨 runtime 使用池需要在主 runtime 创建、并保证主 runtime 最后关闭 | 设计依据来自 tokio 的 IO driver 注册模型（`tokio-1.53.1/src/runtime/io/`；文档锚点 `https://docs.rs/tokio/1.53.1/tokio/index.html` 的 "I/O" 与 runtime 章节）；实现期用集成测试（`runtimes_shutdown_in_reverse_order`）钉住顺序 |

## 5. 其他第三方

| 组件 | 语义 | 出处 |
| --- | --- | --- |
| tokio-util 0.7 | `CancellationToken`：`clone` 共享同一状态，`cancel()` 唤醒所有 `cancelled()` 等待者，`cancelled()` 可安全用于 `select!` | `https://docs.rs/tokio-util/0.7.19/tokio_util/sync/struct.CancellationToken.html` |
| axum 0.8 | `Router::new()`；`axum::serve(listener, router)` 返回 `Serve`，`Serve::with_graceful_shutdown(signal)` 在未来完成/信号触发后等待连接收尾 | `https://docs.rs/axum/0.8.9/axum/fn.serve.html`、`https://docs.rs/axum/0.8.9/axum/serve/struct.Serve.html#method.with_graceful_shutdown` |
| sqlx 0.9 | `sqlx::migrate!("./migrations")` 在编译期要求目录存在（`path.canonicalize()`），把目录里解析出的迁移常量嵌入二进制；空目录 → 空迁移集（不报错） | `sqlx-macros-core-0.9.0/src/migrate.rs`（`expand_with_path` 的 canonicalize 与 `resolve_blocking_with_config`）；文档锚点 `https://docs.rs/sqlx/0.9.0/sqlx/macro.migrate.html` |
| sqlx 0.9 | `sqlite` 特性 = `sqlite-bundled`（bundled `libsqlite3-sys`，不需要系统 SQLite）；`migrate` 特性启用迁移运行器与宏 | `sqlx-0.9.0/Cargo.toml` 的 `[features]` 表 |
| sqlx 0.9 | `Pool::close()` 关闭池并等待连接归还/关闭；`SqliteConnectOptions` 支持 URL 查询参数（如 `mode=rwc`）与 `busy_timeout` | `https://docs.rs/sqlx/0.9.0/sqlx/struct.Pool.html#method.close`、`https://docs.rs/sqlx/0.9.0/sqlx/sqlite/struct.SqliteConnectOptions.html` |
| notify 8.2 | `recommended_watcher(callback)` 在平台原生后端上工作；watcher 必须存活；事件在专用线程回调；`watch(path, RecursiveMode)` | `https://docs.rs/notify/8.2.0/notify/`（`recommended_watcher`、`Watcher::watch`）；实现期编译验证版本与 API 名 |
| tracing-subscriber 0.3 | `reload::Layer` + `reload::Handle::modify`（返回 `Result`，可在线替换 `EnvFilter`）；`EnvFilter::try_new` 校验过滤器字符串 | `https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/reload/index.html`、`https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/filter/struct.EnvFilter.html#method.try_new` |
| serde / toml | `#[serde(default, deny_unknown_fields)]`：未知键报错、缺段取默认；`toml::de::Error` 带 span（用于"字段路径定位"的报错信息） | `https://serde.rs/container-attrs.html#deny_unknown_fields`、`https://docs.rs/toml/1.1.6/toml/de/struct.Error.html` |
| thiserror 2.0 | `#[derive(Error)]` 生成 `Display`/`source` 链 | `https://docs.rs/thiserror/2.0.20/thiserror/` |
| mermaid 11 | 本文档里所有 mermaid 块都用 `mermaid@11` 的 `parse()`（jsdom 环境，本机实测）校验过。两条实测约束：`stateDiagram-v2` 的**转换描述**里不能出现 `::`（改写到带引号的 state label 里可以）；`subgraph` 标题含全角括号时必须写成 `subgraph id["标题"]`。校验脚本是文档工具，不是模板门禁的一部分 | 本机实测（`docs/architecture.md` 的 5 个代码块全部 `OK`） |
