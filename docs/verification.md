# 当前模板验证记录

## 追加记录：handler panic 纳入错误信封（2026-09-15）

- **本次范围**：为 router 装配加 `tower_http::catch_panic::CatchPanicLayer`，把 handler panic 答成统一 JSON 500；`api` 的三条响应契约因此对每条路由成立。下方 2026-09-14 段落保持原样，是当日那次变更的记录，不改写成本次数字。
- **实际环境**：macOS 26.6.2 arm64；Rust 1.97.1，Python 3.14.7。远端 CI 和其他平台没有因此被视为已通过。
- **本次跑过的门禁**：`make check`（含工具测试、默认工程完整 gate、名称矩阵）与 `make verify`（独立空 target）各一次，全部退出 0。actionlint、`python -O` 工具测试和 `panic="abort"` 负向探针本次没有重跑，仍以 2026-09-14 段落为准。

| 检查 | 结果 |
| --- | --- |
| `make check` / `make verify` | 均通过 |
| workspace Rust tests | 每组 98 项通过。本次变更贡献 +1：`api` 新增 panic 信封契约测例，原有 panic 测例改为断言 500 与连接存活。97 → 98；与 2026-09-14 段落的 91 之间的差额来自其后的提交，不是本次 |
| 维护工具 tests | 26 项通过 |
| 真实服务进程 | check 与 verify 各 250 场景通过，覆盖 debug/release × 五布局 |
| 日志 | 每个完整 gate 各 12 场景通过 |
| 依赖 | 未新增第三方 crate；`tower-http` 增加 `catch-panic` feature，`Cargo.lock` 中 tower-http 增加 `futures-util`、`http-body-util` 两条依赖边，两者本已在图中 |

本次模板锁文件 SHA-256：

```text
190e91bab667246383521b3c182385f8018bde97cf923ee4a1a3de5b810b8145
```

原始报告：

```text
/Users/riotian/.cache/axum-starter-template/workspaces/gen-example-service-8sey8ab3/verification.json
/Users/riotian/.cache/axum-starter-template/workspaces/gen-parallel-probe-5op3kzth/matrix.json
/Users/riotian/.cache/axum-starter-template/workspaces/gen-clean-room-service-8f7_peaj/verification.json
```

本机日志：`/tmp/axum-catchpanic-check.log`、`/tmp/axum-catchpanic-verify.log`。

---

## 基线记录：生成内容与实施历史解耦（2026-09-14）

- **日期**：2026-09-14。
- **本次范围**：生成内容与实施历史解耦，release panic 回归夹具归位，以及防止重新泄漏的输出门禁。
- **运行状态**：本地完整回归通过；默认生成与独立空 target 验证均通过。
- **实际环境**：macOS 26.6.2 arm64；Rust 1.97.1，cargo-generate 0.24.0，Python 3.14.7。远端 CI 和其他平台没有因此被视为已通过。
- **历史记录**：各阶段报告保留其当时的名称、路径与测试计数，不将过去的产物重写成现在的布局。它们不随生成项目交付。

## 交付边界

1. 产品源码、注释、测试、配置与 README 不引用实施里程碑或未交付的设计文档。注释就地解释当前的不变量、约束和失败边界。
2. `app/tests/fixtures/release_panic.rs` 是项目自带的回归夹具，不是服务使用示例。生成物不包含 `app/examples` 目录。
3. Cargo manifest 仍显式声明 `[[example]] release-panic-fixture`，用于 `cargo build --release`，并设置 `test = false`、`bench = false`。这不是把夹具排除出生成物，也不是新增服务入口。
4. 夹具保留 `cfg(panic)` 编译守卫，复用实际 TaskSupervisor，检查 panic 被观察、兄弟任务完成收割、runtime 关闭调用返回和预期退出码。不能用普通 test harness 代替真实 release profile。
5. `make gen` 就执行输出审计，而不是必须等到完整 `make check` 才发现残留。源侧审查避免误伤用户自己选择的合法项目名称；非渲染输出必须与源逐字节一致。

## 维护工具

- 默认根：`~/.cache/axum-starter-template/workspaces`；新受管父目录前缀：`gen-`。
- 普通报告：`VERIFICATION_REPORT` → `verification.json`；名称/安全矩阵：`MATRIX_REPORT` → `matrix.json`。
- 临时数据库表、目录、线程与探针输出都按用途命名，不再携带阶段编号。
- 历史受管目录不移动、不覆盖、不自动清理。需要清理时显式提供旧生成根，仍需路径及所有权校验；兼容标识只存在于维护工具，不导出到生成项目。
- 仓库的架构/历史文档与门禁负例可以保留历史词；这不等于允许把它们写入产品源码。

## 验证方式与限制

完整回归沿用 `make check` 和独立空 target 的 `make verify`，包含 Rust 测试、Clippy、名称矩阵、真实进程和日志检查。
新增门禁测试覆盖阶段注释/文件名、未交付设计引用、复制后代码被改写、fixture 错放位置、用户身份不被误判及旧目录清理兼容。
实际平台仍以本次报告为准；不以静态 workflow 定义代替远端 runner 的执行记录。


## 本次结果

| 检查 | 结果 |
| --- | --- |
| `make check` | 默认工程完整 gate、四组名称矩阵、并行/父 workspace/非法名探针全部通过 |
| `make verify` | 独立空 target 重新构建并执行完整生成项目 gate，通过 |
| workspace Rust tests | 默认、四组名称和独立验证每组 91 项通过；不把重复运行累计为独立用例数 |
| 维护工具 tests | 26 项通过；普通模式及 `python -O` 均通过 |
| 真实服务进程 | 默认与独立验证各 250 场景通过，覆盖 debug/release × 五布局 |
| 日志 | 每个完整 gate 各 12 场景通过 |
| 输出审计 | 默认骨架 59 个自有文本源通过；最新注释再次生成并验证 Cargo metadata/feature/tree |
| 用户命名边界 | `m1-service` / `m2` 的显式用户身份能够生成并通过结构检查，审计不会禁止合法用户名字 |
| release 正向 | 中性命名的夹具观察 panic、收割兄弟、返回预期退出码，检查器判定成功 |
| release 负向 | 在受管生成目录以 CLI 临时覆盖 `profile.release.panic="abort"`，构建被夹具的 unwind 编译守卫明确拒绝；manifest 字节未改 |
| workflow | 两份定义经 actionlint 1.7.7 静态检查退出 0（外部 shellcheck 集成未启用） |
| 格式 / 依赖 | fmt 与 diff 空白检查通过；未新增第三方 Rust 依赖，Cargo.lock 不变 |

原始报告：

```text
/Users/riotian/.cache/axum-starter-template/workspaces/gen-example-service-4zhax53b/verification.json
/Users/riotian/.cache/axum-starter-template/workspaces/gen-parallel-probe-azoc4vp5/matrix.json
/Users/riotian/.cache/axum-starter-template/workspaces/gen-clean-room-service-iydksyq1/verification.json
```

本机日志：`/tmp/axum-export-check.log`、`/tmp/axum-export-verify.log`、`/tmp/axum-export-tooling.log`、`/tmp/axum-export-tooling-opt.log`、`/tmp/axum-export-abort-profile.log`、`/tmp/axum-export-actionlint.log`。
日志/受管目录可能被清理，仍可通过上述命令重新生成证据。归档阶段报告保持原样，不将其中旧文件名改写为本次文件名。

模板锁文件 SHA-256：

```text
029b527e16a88bd02622b201c781a9855bf5225ac01b19ccca7f075c198b8eeb
```

本次没有提交或推送，没有修改既有生成工程，也未修改参考项目。正常服务的任务、配置、存储和关停协议没有改变；变化集中于注释/命名/夹具组织及模板输出审计。
