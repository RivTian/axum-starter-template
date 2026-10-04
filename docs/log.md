# 实施日志 · 第六版

> 任务书：[prompts/01-design-rust-template-v6.md](prompts/01-design-rust-template-v6.md)。每个阶段一节，每节不超过一屏（任务书 §8.5）。

## P0 · 骨架

**做了什么**

- 模板工程：`template/cargo-generate.toml`、hooks、许可证文件、`deny.toml`、`typos.toml`、`rust-toolchain.toml`、`rustfmt.toml`、`.genignore` 移植自 V4。
- 9 个 crate 的骨架：`util`、`domain`、`config`、`runtime`、`telemetry`、`api`、`infra`、`test-utils`、bin。库名为 `svc_<role>`，bin 的库名为 `svc_app`。workspace lint 在 V4 的基础上加了 `print_stdout`、`print_stderr`（deny）和 `unreachable_pub`（warn）。
- 架构守卫 `crates/<name>/tests/architecture.rs`：§5.9 的 7 条守卫按名字报告，每条有一个反例测试，另有"proc-macro 不展开"的测试。
- 维护者工具：`justfile`、`scripts/render.sh`（移植）、`scripts/check.sh`（提交档）、`scripts/template-lint.py`（由 V4 的 16 条编号规则收敛为 7 条按名字的规则，并加入文件规模、`mod.rs`、嵌套深度检查）、`scripts/matrix.tsv`（5 个组合，新增 m5：`None` + Docker + 无 CI）。

**检查**

| 命令                     | 结果                                                                    |
| ------------------------ | ----------------------------------------------------------------------- |
| `just lint`              | 0 条发现；self-test 7 条规则全部命中                                    |
| `just check m2`          | 通过（fmt、clippy ×2、nextest 9/9、doctest、doc、typos、machete、deny） |
| `just check m1 m3 m4 m5` | 全部通过                                                                |

**偏离任务书**

- 生成项目的 `just check` 暂不含 `workflows`：P0 还没有 workflow 文件，P2 加入 CI 时一并加回。
- 守卫中"上游名字"一条除了 `Custom`、`CustomCode`、`ReusedOnly`，还禁止 `new_str` 与 `ErrorType::new(`：这两个构造函数会产生 `Custom` 变体，V4 审查 C-11 指出它们能绕过规则。
- `CARGO_MANIFEST_DIR` 在运行期读取（`std::env::var`），而不是编译期的 `env!`，避免共用 target 目录时描述另一个副本（V4 阶段 C 的发现）。

**超出规模预算**

- `tests/architecture.rs` 465 行，软上限 300 行。其中常量表约 70 行（rustfmt 把列表展开为一项一行），反例测试约 150 行；判定与描述部分约 220 行。未超过 500 行的硬上限。

**未验证**

- cargo-generate 0.25.0（最新版）的渲染一致性：本机只有 0.24.0，P2 的矩阵阶段处理。

## P1 · 内核

**做了什么**（提交 `24c68cd` 至 `5e3598d`）

- util：vendor pingora-error（上游 `4487f7b`），新增 `ErrorType::Kind(&'static ErrorKind)`；`Class` 与 `ErrorType::class()`；原因链与日志字段；`log_at_level!`、`log_error!`。`scripts/vendor-pingora-error.py` 写入与证明补丁。
- domain：`todo/{model,usecases,ports,events,error}.rs` 与 `clock.rs`。
- runtime：`Service` 契约；监督器 = 纯状态机 `supervisor/table.rs` + 驱动 `supervisor.rs`（单一有序通道）；信号、阶段、就绪、事件总线、配置段。
- infra：内存仓储、系统时钟、`BusTodoPublisher`、后台服务 `EventLog`。
- api：problem 渲染与映射（`problem.rs`、`problem/status.rs`）、todo 端点、探针、中间件、`HttpServer`。
- test-utils：仓库替身（失败、变慢、panic）与契约、时钟、发布者、脚本化服务、手发信号、日志捕获。
- telemetry：最小版本（stdout、text/JSON、一个过滤器）。bin：默认配置、装配、退出码。

**检查**

| 命令或操作                                        | 结果                                                                                       |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `just check`（5 个组合）                          | 全部通过；nextest 74 个测试，doctest 5 个                                                  |
| `scripts/vendor-pingora-error.py --check-patches` | 通过：vendored 文件 = 上游 + 列出的补丁                                                    |
| 植入：去掉内存仓储的版本比较                      | 契约测试失败：`broken rule: update at a stale version reports the current version`；已恢复 |
| 冒烟：运行二进制，请求探针与端点，发 SIGTERM      | 排空 5 s 后停止，退出码 0；日志顺序符合状态表                                              |
| 冒烟：端口已被占用                                | 退出码 69；`service failed` 只记一次，`phase changed` 的 `reason` 写明原因                 |

**偏离任务书**

- vendor 布局：上游两个文件不合并，放在 `error/pingora.rs` 与子模块 `error/pingora/immut_str.rs`。补丁更少，证明更简单；lint 属性只传给上游自己的子模块。
- 守卫的"上游名字"再加 `new_code`（同样会构造 `CustomCode`）。`ErrorType::class()` 对未列出的变体用兜底分支归为 `Internal`，这样本模板的代码不必写出 `Custom` 等名字。
- 状态表细化：已调用 `ready()` 的后台服务在 Starting 阶段正常返回不算启动失败，否则结果取决于它与其他服务就绪的先后（C-1 一类的不确定）。
- 进入 Stopping 时，阶段日志的 `reason` 写错误的上下文；排空超时的日志字段是仍在运行的服务（`services`），不是 `in_flight`。HTTP 服务不再计数在途请求，`ServiceContext` 没有 `deadline()`，截止时由监督器中止任务。
- 内存仓储不再为"锁中毒"设专门的错误类型和健康检查：临界区只做整值替换，中毒后直接恢复（§10 第 2 条）。`/readyz` 的 `checks` 因此为空。
- 请求超时在 problem 中间件里实现，回答 503 并带 `Retry-After: 1`（超时错误标为可重试）。
- 提前完成附录 A 的两条：客户端取消的请求也记 `request finished`（`cancelled = true`）；响应序列化失败回答 500 problem。

**超出规模预算**

- 监督器代码（不含测试）483 行，软上限 450 行。其中状态表 235 行，驱动 248 行，两者都以文档注释说明每个分支。
- 当前生成项目非测试代码 4356 行（含 vendor 约 620 行代码），测试代码 1910 行。

**流程**

- 一次 `just commit | tail` 管道吞掉了失败的退出码（rustdoc 的私有链接），提交照样发生；已修复并 amend。之后的命令都开 `pipefail`。

**未验证**

- `HEAD` 与 hyper 层的拒绝、`check-config` 等附录 A 其余条目：属于 P2。
- cargo-generate 0.25.0：同 P0。
