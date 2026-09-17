# 验证记录

> 只记录**实际跑过**的东西。没跑过的不写成跑过；本地绿不等于远端或其他平台通过。

## 1. 环境

| 项 | 值 |
| --- | --- |
| OS | macOS 26.6.2 (25G83)，arm64 |
| rustc | 1.95.0 (59807616e 2026-04-14) |
| cargo | 1.95.0 (f2d3ce0bd 2026-03-21) |
| rustfmt | 1.9.0-stable (59807616e1 2026-04-14) |
| cargo-generate | 0.24.0（`cargo generate-generate 0.24.0`） |
| python3 | 3.14（仅模板侧脚本用：矩阵/探针/切片） |
| 机器 | 本机（Apple Silicon），未使用容器或远端 |

## 2. 跑过的门禁与结果

全部在模板仓库根执行；一次完整的 `make gate` 实测 **4m32s**（real），`user 12m12s`。

| 目标 | 内容 | 结果 |
| --- | --- | --- |
| `make audit` | 模板树：顶层白名单、生成面无垃圾/无符号链接、ignore 条目存在（3 条）、迁移目录为空、`.liquid` 名单、渲染面占位符合法（用到 5 个）、派生变量由 pre hook 提供 | ok（7 项） |
| `make slices-check` | README 的 6 个切片代码块与 `scripts/slices/` 答案文件逐字一致 | ok |
| `make template-fmt` | `tools/audit` 自身 `cargo fmt --check` | ok |
| `make matrix` | 前缀夹具 9 组（4 组合法生成成功 + 5 组非法被拒且消息指向 `crate_prefix`）；名字矩阵 `typical` 生成 + `cargo fmt --check` | ok（10 项） |
| `make check-gen` | 极端长名 + 极端短名各一次真实换名生成：完整 `make check`（fmt-check / lint / test）+ 生成结果审计（61 个文件） | ok（2 项） |
| `make probe` | 真实进程：构建串、两个任务的 `stopped`（任务名 + runtime）、配置落在可执行文件旁、两个 cwd 读同一份配置、SIGINT 后退出码 0 | ok |
| `make slices` | 两条 README 切片照抄进生成项目：`make check` 绿 + 进程内行为断言（`flush` tick / `notes written` / `stopped`） | ok（2 项） |
| `make gate` | 以上全部按序 | `gate: all green` |

### 生成矩阵的具体身份

| 用例 | `--name` | `crate_prefix` | 做了什么 |
| --- | --- | --- | --- |
| long | `extremely-long-project-name-used-for-template-verification`（58 字符） | `abcdefghijklmnopqrstuvwxyz012345`（上限 32 字符） | 完整 `make check` + 生成结果审计 |
| short | `x` | `s` | 完整 `make check` + 生成结果审计 |
| typical | `dev-service` | `svc` | 生成 + `cargo fmt --check` |

每个生成目录都在仓库树外（`mktemp -d`），各自用自己的 `CARGO_TARGET_DIR`（不复用编译缓存）。
长名用例的 32 字符前缀是刻意选的上限值：它同时验证"rustfmt 排版与名字解耦"这条纪律
（生成结果 `cargo fmt --all -- --check` 干净）。

### 生成侧测试

| 项 | 数字 |
| --- | --- |
| 生成项目 `cargo test --workspace` 通过用例 | 104（含 doc-tests 0 条） |
| 其中内核测试（runtime） | 32（`tests/supervisor.rs` 17 + `tests/shutdown.rs` 6 + 单元 9） |
| 其中结构纪律测试（app） | 7（`tests/structure.rs`） |
| `tools/audit` 自身单元测试 | 2 |

### 编译与工具耗时（本机实测）

| 操作 | 耗时 |
| --- | --- |
| 生成项目的首次 `cargo build`（全新 target） | real 20.6s（user 91.2s） |
| `cargo test --workspace --no-run`（在首次构建之后） | real 8.0s |
| `cargo fmt --all -- --check` | real 0.06s |

`sqlx` 的 SQLite 走 bundled（`sqlite-bundled`），本机没有系统 SQLite 依赖参与。

## 3. 与设计文档的对照（实现期修正，已回填）

| 项 | 设计预期 | 实测/处理 |
| --- | --- | --- |
| rustfmt 名字相关性 | `.rs` 过 Liquid + 前缀上限 32 + 上限长度重跑 fmt 门禁 | 门禁在 32 字符前缀上先红：`use` 列表换行、导入组顺序随前缀首字母变化。修法是**结构性**的：导入分四组（std / 第三方 / 兄弟 / 本地，组间空行 + rustfmt 不跨组重排）、兄弟导入一项一行、名字进常量（clap 属性），之后上限前缀下 `cargo fmt --check` 干净 |
| 关停预算的时间基准 | L1 `total` 约束整段关停 | 实现时发现原来的 `hard_deadline` 从 `run()` 开始计时（长运行的服务会吃掉预算）→ 改为从**进入关停**计时，`report.elapsed` 同步修正 |
| L0 看门狗（15s） | 超时打印并强推 | 原实现从进程启动计时 → 改为从**停止请求**之后计时（L0 是关停的上限，不是进程寿命的上限） |
| 日志形态 | 只有一种 | 追加"关闭 ANSI 颜色"（日志文件/采集器不需要转义序列），probe 的构建串断言因此可以逐字匹配 |
| "不预建表"的落点 | 骨架不预建表 | 原实现在生成项目里放了一条"迁移集必须为空"的测试，会拦住用户照 README 加的第一个迁移 → 改为模板侧 `make audit` 检查；生成结果不再携带模板纪律 |
| 生成侧 `db` 测试 | 迁移记账 | 去掉对迁移条数的断言（加迁移之后仍要绿） |

## 4. 没有验证的东西（明确列出）

- **Windows / Linux**：所有测试与门禁只在 macOS arm64 上跑过。`.liquid` 遮蔽在 `--git` 模板路径上
  依赖 `fs::rename` 的替换语义，只在 macOS 上实测（见 `docs/architecture.md §10.2`）。
- **MSRV**：`rust-version = "1.85"` 是 edition 2024 的下限标注；实际只在 rustc 1.95 上验证。
  本机还有 1.91 与 1.97 toolchain，但没有把它们跑进门禁（不在本文声称）。
- **cargo-generate 0.24 之外**：只实测 0.24.0；`cargo-generate.toml` 把兼容区间写成 `>=0.24.0, <0.25.0`。
- **强杀路径**：L0 看门狗触发时 `process::exit(4)` 的进程级行为没有自动化证据（测的是时序逻辑）。
- **更长的项目名**：极端长名用了 58 字符；更长的名字只影响 README 标题、二进制名与 `[[bin]]`（不经过 rustfmt）。
- **`--git` 模板路径**：`make gate` 用 `--path` 本地模板。手工补跑过一次 `--git`（本地仓库路径 → clone）：
  生成成功、`.liquid` 遮蔽生效（结果里 0 个 `.liquid`）、`make check` 退出码 0、生成结果审计通过。
  只跑了一次（`gitcheck` 前缀、单名字），没有进门禁矩阵。
