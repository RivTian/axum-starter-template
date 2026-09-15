# M1：工程基线与高风险验证记录

> 历史记录：以下描述 M1 验收时的切片。当前代码已推进至 M2，最新状态见 `docs/m2-verification.md`；本页不作为当前功能缺失清单。

- **日期**：2026-09-14。
- **结果**：M1 本地验收完成；M2 尚未开始。
- **范围**：生成与编译基线，以及 JoinSet 跨 runtime、空迁移、真实 release panic、HTTP 强制终止边界四项技术验证。
- **重要限制**：这些是组件/运行时机制证明，不是完整 TaskSupervisor、信号处理或 Web 服务生命周期的实现验收。
- **代码状态**：当前工作树增量，未提交、未推送；没有整体恢复备份分支，没有修改 `edge_dev`。

## 1. 实际交付

仓库根目录：`/Users/riotian/Documents/code/axum-starter-template`。

| 部分 | M1 已实现 | 尚未声称实现 |
| --- | --- | --- |
| workspace | 五 crate、单向允许边、稳定内部依赖别名、锁定工具链/依赖、release unwind、禁止自有 unsafe | 不为未来装配提前声明无人消费的依赖 |
| core | BuildInfo、非零 TickerInterval 值类型 | 完整配置模型、watch 快照、生命周期发布器 |
| worker | 固定周期 ticker future；目标 runtime 内构造 timer、完整首拍、Skip、取消优先 | 启动提交门、热配置消费及完整周期范围校验 |
| storage | SQLite 单池、StorageOwner 保留初始化/关闭权、health-only `Arc<dyn Storage>`、空嵌入迁移、自检 | 完整文件配置、生产生命周期预算、真实业务迁移、并发迁移与中断恢复验收 |
| api | 不自 spawn 的 HTTP transport future；真实连接测试 | 三个系统端点、AppState、提取器/错误信封、HTTP 配置及应用启动门 |
| app | 正常可编译的主程序，支持 `--version` / `--help`；独立测试和 release 示例 | 正常服务启动；TaskSupervisor、启动/关停协调器、OS 信号与 runtime 配置解析 |
| 工程链 | cargo-generate、两个 Makefile、独立模板/项目 CI、命名/依赖/后端检查、结构化 lock 映射、源码格式回写保护、生成目录清理保护 | 远端 CI 已绿、全平台支持或完整 M5 产品化验收 |

主程序默认执行会明确返回 **1** 并说明 M2 服务装配未实现，不启动半成品 HTTP，也不把打印一行启动日志称为服务可用。
故障注入只存在于测试和单独的 release example；生产主程序没有 panic 开关或额外测试路由。

## 2. 固定的工具与依赖

| 项目 | 采用/验证值 |
| --- | --- |
| 本地平台 | macOS / arm64（Darwin arm64） |
| Rust / Cargo / 声明 MSRV | 1.97.1；是已验证的基线，不宣称最低可能版本 |
| Rust edition / resolver | 2024 / 3 |
| cargo-generate | 0.24.0 |
| Python 实测 | 3.14.7；脚本使用 Python 3.11+ 标准库接口 |
| 本地 C 编译器 | Apple clang 21.0.0（构建 bundled SQLite 所需） |
| Tokio / Tokio-util | 1.53.1 / 0.7.19 |
| axum | 0.8.9；M1 仅开启 http1、tokio |
| sqlx | 0.9.0；runtime-tokio、sqlite-bundled、migrate、macros |
| thiserror / tracing / tempfile | 2.0.20 / 0.1.44 / 3.27.0 |
| CI checkout | 固定为官方 v4 引用在核验时的提交 `11d5960a326750d5838078e36cf38b85af677262` |

模板 `Cargo.lock` SHA-256：

```text
a2a4474536558e36f6d7de68558e3e7a3b7f92a0d60a65a22b78ba093d9e0538
```

不是从备份复制锁文件，而是重新解析本次依赖组合，再只映射 workspace 身份。
`macros` 因 `migrate!` 必须开启，会带入部分 derive/offline 支持；没有使用 SQL 查询宏，也没有生成 `.sqlx` 离线查询缓存。

Linux workflow 已编写，但未推送/触发远端 Actions，因此**没有将 Linux 或 Windows 记为已验收平台**。

## 3. 四项高风险验证结果

### 3.1 JoinSet 跨 runtime 与失败分类：通过

证据代码：

- `/Users/riotian/Documents/code/axum-starter-template/app/tests/runtime_semantics.rs`

已验证：

1. 主 runtime 上的 JoinSet 直接持有 spawn_on 到独立单 worker runtime 的真实任务，没有 watcher 包装层。
2. 任务内部的 timer 实际运行，返回目标线程名；退出 id 与登记时的 AbortHandle id 一致。
3. future 返回的错误与 JoinError panic 可区分，且都能找回原任务 id。
4. 异步 pending 任务在第一段等待到期后可 abort，并在第二段有界等待中确认取消。
5. 已经开始的 blocking 作业不能靠 abort 结束；第二段等待会到期。测试自己随后释放门闩并 join，避免污染测试进程。
6. child token 不能取消 parent 或 sibling；根取消会级联。空 JoinSet 立即返回 None。

**未据此宣称**：完整的重名拒绝、停止后禁止注册、启动提交、全局 deadline、首因保存或完整 TaskSupervisor 已实现。
这些仍在 M2/M4 按架构文档实现。

### 3.2 空迁移与初始化所有权：通过

证据代码：

- `/Users/riotian/Documents/code/axum-starter-template/storage/src/lib.rs`
- `/Users/riotian/Documents/code/axum-starter-template/storage/src/tests.rs`
- `/Users/riotian/Documents/code/axum-starter-template/storage/build.rs`

使用每测试独立的临时文件库：

- 空业务迁移集能执行连接、Migrator 和 health；只出现 `_sqlx_migrations`，记录数为 0，无业务表。
- 同一文件数据库关闭后重新初始化成功，外键选项开启。
- lazy pool 构造成功但文件路径不可用时，initialize 返回 connect 错误，不发布门面。
- 故意占满单连接池，取消正在等待的初始化 future 后，外层 StorageOwner 仍然可以显式关闭池。
- API 能拿到的 trait 只有 health；关闭能力没有泄漏到 `dyn Storage`。

补充验证：维护脚本只在本次新生成的受管测试目录内添加、修改、删除一条临时迁移，逐次检查 Cargo 的 storage library artifact `fresh` 标志。
未变时 fresh；三种目录变更都导致重新编译。测试最后删除临时 SQL，模板的生产迁移目录仍只有 README。
这证明重编译跟踪生效，不等于真实业务迁移的锁竞争或中断恢复已验收。

### 3.3 真实 release panic：通过

证据代码：

- `/Users/riotian/Documents/code/axum-starter-template/app/examples/m1_release_panic.rs`
- `/Users/riotian/Documents/code/axum-starter-template/app/tests/check_release.py`

不是 `cargo test --release` 冒充普通 release profile，而是：

1. `cargo build --locked --release --example m1-release-panic` 生成普通 release 可执行文件。
2. 从 Cargo JSON 构建消息取得实际 executable，再在独立进程运行，设置外部时间上限。
3. 任务 panic 被 JoinError 观察到；根取消后 sibling worker 被 join，Drop 标记确认执行。
4. 在同步 main 中关闭 runtime，最终程序返回 **1**，不是成功返回 0。
5. 同时捕获预期的 panic hook stderr；若 profile 改为 abort，该 fixture 的编译约束直接使门禁失败。

必须依次出现的 stdout：

```text
M1:panic-observed
M1:worker-joined
M1:runtime-teardown-returned
```

这验证的是 release unwind 与监督/清理机制的可行性。完整服务自身的 panic/关停验收，仍需在 M2 的真实协调器上重复。

### 3.4 HTTP 外层 abort 与连接残留：通过

证据代码：

- `/Users/riotian/Documents/code/axum-starter-template/api/src/lib.rs`
- `/Users/riotian/Documents/code/axum-starter-template/api/src/tests.rs`

两种情况都使用真实 loopback 临时端口和显式请求握手，不 sleep 猜就绪：

- **正常**：handler 被门闩保持在途时，graceful 服务 future 不完成；放行请求后才完成，客户端收到完整 200 响应。
- **强制**：HTTP 在 extra runtime 内创建 listener；根取消后 abort 并 join 外层 serve，handler 的 Drop 标记仍为 false。
  保持客户端连接且不放行 handler，关闭它所属的 extra runtime 后，Drop 标记才变为 true。

因此保留原设计：**outer join 不是库内连接已排空的证明**。M2 的 forced 路径不能据此进入正常“所有使用者已停、可以安全 close/flush”的分支。
这不是要求在同一进程强关全部连接后继续服务；本版仍以失败退出结束强制路径。

## 4. 工程链调整与试错记录

### 4.1 稳定 Rust 依赖别名

包名仍为 `<prefix>-core` 等；workspace 将它们以 `service-core` 等固定依赖键引入，Rust 使用 `service_core` 等固定导入名。

只对 Cargo manifest、锁文件和项目 README 做 Liquid 展开，Rust 原样复制。
因此 `make fmt` 无需把具体项目名再替换回占位符；按实际源码路径回写前，还会拒绝覆盖期间已发生变化的源文件。
这项调整已同步架构文档，不改变五 crate、单向依赖或组件门面。

### 4.2 锁文件身份映射与名称碰撞

不执行“全文件把某个前缀替换掉”的操作：只重命名无 registry/git source 的五个 workspace package，外部包名和 checksum 保留。
依赖引用补足版本及 source 来消歧。

实际使用 `crate_prefix=sqlx` 验证：本地 `sqlx-core 0.1.0` 和外部 `sqlx-core 0.9.0` 同时存在，仍可 `--locked` 编译、测试和运行 release fixture。
这不是只用字符串单测声称碰撞场景可用。

### 4.3 单后端门禁的误报修正

初版检查直接根据 metadata resolve nodes 中是否有 sqlx-mysql/postgres 判错，第一次门禁因此失败。
核验实际 workspace 构建树后确认，它们属于解析/锁文件的可选依赖记录，而不是本次启用的驱动。

已改为同时核对：

- 五个 workspace 的真实依赖边和内部别名；
- sqlx 当前启用的 feature 集；
- `cargo tree --workspace --locked --edges normal,build,dev` 的实际依赖树。

选中树不含 PostgreSQL/MySQL；显式关闭 Any、加载扩展、deserialize、unlock-notify 等本阶段无消费者能力。
不以 metadata/锁文件中有记录就推断已编译，也不因为一次误报就移除后端门禁。

### 4.4 生成物安全与 CI 分离

- 每次在树外创建唯一目录，无 overwrite、无“当前生成目录”共享指针。
- clean 仅接受匹配标记的受管目录，拒绝仓库/家目录等危险位置、标记不匹配和 symlink 路径。
- 工具检查使用显式异常，在 `python -O` 下也保留门禁。
- 模板 CI 留在源码 `.github/`，不进入生成结果；生成项目不附带 workflow，结构门禁断言其没有 `.github`。
- 沿用原 `.gitignore` 其余规则，去除旧安装根假设，补 SQLite 文件/边车和 Python 缓存模式。

## 5. 实际执行的门禁

| 命令/场景 | 结果 |
| --- | --- |
| `make check`：m1-project / m1x | 通过 |
| `make verify`：m1-clean-room / verifyx，独立空 target | 通过 |
| collision-project / sqlx | 通过 |
| a-deliberately-long-project-name-for-the-m1-generation-matrix / long-prefix-for-format-safety-m1 | 通过 |
| 生成项目自身的 `make check` | 通过，未只测试模板驱动器的等价命令 |
| 两份 CI YAML 的语法、触发事件和只读权限 | 通过静态解析；不等于远端 Actions 已执行 |
| Python 工具单测 | 11 项通过；普通模式与 `python -O` 均通过 |
| 每组生成 workspace 的 Rust 测试 | 14 项通过；无 ignored/跳过项 |
| release 进程 fixture | 按预期观察 panic、收割 sibling、同步 teardown、退出 1；检查器返回成功 |
| 迁移增/改/删重编译检查 | 四组生成项目均通过；临时 SQL 已清理 |

每组完整 gate 包含 fmt、Clippy（全部目标且拒绝警告）、workspace 测试、workspace 构建、release fixture、版本/非服务启动 smoke 和结构校验。

复现命令（从模板根执行，不直接在模板根运行 cargo build）：

```sh
make check
make verify
python3 scripts/template.py check --name collision-project --prefix sqlx
python3 scripts/template.py check \
  --name a-deliberately-long-project-name-for-the-m1-generation-matrix \
  --prefix long-prefix-for-format-safety-m1
python3 -O -m unittest discover -s scripts -p 'test_*.py' -v
```

本次 JSON 结果位于以下受管目录；它们是本地运行证据，不是生成项目的部署路径，清理缓存后可用上述命令重新生成：

```text
/Users/riotian/.cache/axum-starter-template/m1/m1-m1-project-acf2y1at/m1-result.json
/Users/riotian/.cache/axum-starter-template/m1/m1-m1-clean-room-6ize5kq7/m1-result.json
/Users/riotian/.cache/axum-starter-template/m1/m1-collision-project-aj0m9pkh/m1-result.json
/Users/riotian/.cache/axum-starter-template/m1/m1-a-deliberately-long-project-name-for-the-m1-generation-matrix-q2gcajgi/m1-result.json
```

## 6. M2 交接边界

M1 没有发现必须推翻已确认 P0 的证据，下一步按架构文档执行 M2：

- 实现 app 私有 TaskSupervisor，而不是把测试中的 JoinSet 探针当作完整实现。
- 接上配置加载、启动资源所有权、启动确认/提交门、OS 信号、统一 deadline 清理与退出码。
- 装配三个系统端点、最小 AppState、错误/提取器契约，并将 HTTP force 证明门槛写进实际关闭分支。
- 把 M1 固定参数的组件连接起来，补充真实服务上下文里的配置校验、错误适配和生命周期测试。

watch 热重载仍是 M3，可配置多 runtime 的完整构建/部分失败回滚/统一关闭预算仍是 M4。
架构文档的 55 项完整验收矩阵没有被标记为全部通过；远端 CI、跨平台、真实业务迁移恢复和部署准备也没有借 M1 名义被宣称完成。
