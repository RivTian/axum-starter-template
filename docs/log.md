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

## P2 · 补全

**做了什么**（提交 `3be7369` 至本节所在的提交）

- 配置：各 crate 的配置段加上 serde 与取值校验（`svc_util::de`）；config 移植 V4 的加载语义，按来源拆成 `load/{layered,file,env,keys}.rs`；返回 `Report`（数据）；`check-config` 的表格在 `table.rs`。
- telemetry：stdout（text/JSON、颜色）与滚动 gzip 文件各有过滤器；文件层用自己的字段格式化器类型（C-37）；再次 `init` 在碰文件之前报错；panic 钩子最先安装；`console` feature。
- bin：CLI（`run`、`check-config`、`probe`）、版本信息（vergen-gitcl）、退出码 64 与 78、`config/example.toml` 与默认值一致的测试；进程级测试 11 个。
- 生成项目：Dockerfile、三个 workflow、`just` 配方、README、`docs/architecture.md`（191 行）。
- 模板仓库：`check.sh --full`、`names.sh`（新增"与依赖重名"规则）、`version-info.sh`、`template-ci.yml`、维护者 README。

**检查**

| 命令                       | 结果                                         |
| -------------------------- | -------------------------------------------- |
| `just check`（5 个组合）   | 全部通过；m2 共 120 个测试                   |
| `just full m1`、`m4`、`m2` | m1、m4 全部通过；m2 除 Docker 三步外全部通过 |
| `just names`               | 全部通过（含 `tokio`、`http-body` 被拒）     |
| `just version-info`        | 15 个场景全部通过（首次提交前为 `unknown`）  |
| `just lint`、actionlint    | 通过                                         |

**偏离任务书**

- 校验器模块命名为 `svc_util::de`，不叫 `settings`：词表中 `settings.rs` 专指本 crate 拥有的配置段。
- 加载器去掉了 V4 的 `checks` 函数指针钩子：过滤串由 telemetry 的反序列化器校验，在"逐键定位"中与其他问题一起报出。`RUST_LOG` 不再写死，由 bin 以别名表传入。
- 非 UTF-8 的路径参数回答固定的 `detail`（`invalid path parameter: not valid UTF-8 text`）。hyper 在路由之前拒绝的请求无法成为 problem，已在生成项目文档中写明不支持。
- 项目名不得与模板的依赖重名（`tokio` 会让 `[workspace.dependencies]` 出现两个同名键），也不得使派生的 crate 名与依赖重名（`http-body` 加 `-util`）。
- `Secret` 与加载器的密钥脱敏保留：默认配置里没有密钥，但这是加数据库连接串时的扩展点。
- 配置、日志与 CLI 合成一个提交：三者相互依赖，拆开后中间提交无法通过检查。
- 全量档中镜像用 `rs-starter-template-check-<name>` 标签，不经过 `just docker-build`（它打 `<name>:dev`），以免触碰用户已有的镜像。

**超出规模预算**

- 生成项目非测试代码 6533 行（V4 约 7350 行），软上限 5500 行。主要来自：config 加载器 549 行、telemetry 723 行（滚动文件 334 行）、util 的校验器与时长 551 行、vendor 约 620 行。测试代码 3424 行，在 5000 行以内。
- 加载器（`load.rs` + `load/`）549 行，在 800 行以内；维护脚本合计约 1020 行，在 1200 行以内。

**流程**

- 一次在后台全量档运行期间修改了 `check.sh`；那次结果未受影响（退出码 0），之后用新脚本重跑了 m1 的全量档。
- api 的测试用到暂停时间，但 api 没有声明 tokio 的 `test-util`，之前靠 workspace 的 feature 合并才编译通过；已显式声明。

**未验证**

- Docker：本机 Docker 守护进程（OrbStack）没有运行，m2、m3、m5 的"无 lock 文件时镜像构建报错""镜像构建""镜像冒烟"三步未执行。没有擅自启动守护进程。
- cargo-generate 0.25.0：本机只有 0.24.0，`just compare` 无法运行；模板 CI 会安装两个版本。
- 模板 CI 尚未在 GitHub 上运行；workflow 中 Actions 的版本沿用 V4（如 `actions/checkout@v4`），未核对当前最新的主版本。

## P3 · 审查与交付

**做了什么**（提交 `79fb9fb`、`c2b9e39` 与本节所在的提交）

- Docker 补跑：OrbStack 启动后，m2、m3、m5 的镜像三步全部通过；用户原有 53 个镜像前后一致。
- 3 个只读子 agent 分别审查运行时与 HTTP、配置与日志、模板工程，共报约 25 条；主会话复现了其中的关键条目后修复。

**审查发现的处理**

| 发现                                                                     | 处理                                                                                                    |
| ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| 服务就绪后立即失败，退出码在 69/70 间随机（多线程下 300 次复现两种结果） | 修复：按失败服务自己是否已就绪判定（未就绪 69，已就绪 70）；新增多线程测试，每个场景 300 次只有一种结果 |
| 暂停时间下的 50 次循环跑在单线程上，证明不了确定性                       | 去掉这些循环，确定性由上面的多线程测试保证                                                              |
| 状态机里有日志调用；后台服务结束记两行                                   | 状态机改为纯函数，日志只在驱动里记                                                                      |
| 外来错误的文本没有记录（如 "Address already in use"）                    | 新增 `error.cause` 字段；日志启动失败的 stderr 也带上原因                                               |
| 不到 HTTP 的错误种类也带标题                                             | `ErrorKind::new(name, class)`，标题改为可选的 `.titled(..)`                                             |
| 未设置的 `Secret` 被加载成字面量 `<redacted>`                            | 删除 `Secret` 与加载器的脱敏逻辑（默认配置里没有密钥）；文档写明配置值原样显示                          |
| 文件名、变量名、键中的换行能伪造输出行                                   | 键与来源中的控制字符一律转义                                                                            |
| 只有逗号的过滤串通过校验并关掉全部日志                                   | 拒绝没有指令的过滤串                                                                                    |
| 空的 `<PREFIX>_CONFIG` 被当作路径                                        | 视为未设置                                                                                              |
| 生成项目 CI 的 `console` job 未安装 nextest                              | 补装                                                                                                    |
| 项目名与间接依赖同名（如 `hyper`）时 `-p` 有歧义                         | 配方与 Dockerfile 改用 `--bin`；已渲染名为 `hyper` 的项目验证                                           |
| 全量档的 `CARGO_TARGET_DIR` 检查与默认目录相同，测不出问题               | 改为另一个目录                                                                                          |
| 守卫的 HTTP 闭包规则没有反例测试；注释与检查范围不符                     | 补测试；闭包只跟随 normal 依赖，注释写准                                                                |
| template-lint 漏掉无连字符的编号与 `#L` 行号                             | 补上                                                                                                    |
| 同名文件多义（`table.rs`、`report.rs`、`health.rs`）                     | 改名为 `supervisor/machine.rs`、`error/fields.rs`、api 的 `probes.rs`                                   |
| 状态表缺两个格子的测试；排空超时只记一行 warn 无测试；panic 钩子无测试   | 补测试                                                                                                  |
| 文档：`mod.rs`、500 行、`phase=running`、新配置键的写法                  | 改正；补"新配置键必须用 `svc_util::de`、不用 `Option`"与"就绪检查"的落点                                |
| `request finished` 的状态码在 span 与事件中各出现一次                    | 只留在事件中                                                                                            |

**按设计保留或只写进文档的**

- 请求头始终发不完的客户端会让停止拖到截止时间并以 75 退出：axum 的 `serve` 不提供读请求头的超时，生成项目文档已写明。
- panic 记两行：一行是 panic 本身（带位置），一行是它的后果（`request failed` 或 `service panicked`）。
- 文件中超出机器范围的裸整数是 TOML 语法错误，报为"不是合法的 TOML"，不是"超出范围"；环境变量、命令行与带引号的值仍报"超出范围"。
- `HealthRegistry` 目前没有注册任何检查：它是就绪检查的扩展点，文档的"加一个功能"表已列出。
- bin 的 `lib.rs` 含 `main` 的分派（约 10 行），偏离"`lib.rs` 只声明模块"；文档注释已改为如实说明。
- util 的 `error.rs` 是错误类型本身的门面，不是错误种类常量，是词表的一个例外（公开路径 `svc_util::error::Error` 需要它）。
- 配方名沿用 V4 的 `run` 与 `docker-build`，不是 §5.10 写的 `dev` 与 `docker`。

**检查**

| 命令                    | 结果                                                     |
| ----------------------- | -------------------------------------------------------- |
| `just full`（5 个组合） | 全部通过，含镜像构建、镜像冒烟、无 lock 文件时的两处报错 |
| `just names`            | 全部通过                                                 |
| `just version-info`     | 15 个场景全部通过                                        |
| `just lint`             | 0 条发现；self-test 7 条规则全部命中                     |
| 用户的 Docker 镜像      | 前后一致（53 个）                                        |

**规模**

- 生成项目非测试代码约 6400 行（V4 约 7350 行；软上限 5500 行），测试代码约 3600 行。
- 监督器：驱动 254 行，状态机 236 行（测试另在 `machine/tests.rs`）；合计 490 行，超出软上限 450 行。
- 架构守卫 480 行，超出软上限 300 行；`docs/architecture.md` 199 行；维护脚本、justfile 与模板 CI 合计约 1150 行。

**未验证**

- cargo-generate 0.25.0 的渲染一致性（`just compare`）：本机只有 0.24.0。
- 模板 CI 与生成项目的 workflow 尚未在 GitHub 上运行；Actions 的主版本沿用 V4。

## 补充 · 本地运行模板 CI

- 根 `justfile` 新增 `just ci`：依次运行 `lint`、`full`、`compare`、`names`、`version-info`，与 `template-ci.yml` 一致。维护者 README 写明了用法。
- cargo-generate 0.25.0 装在临时目录 `/tmp/rs6-cg/0.25.0`，未改动 `~/.cargo` 中已安装的工具。
- `just compare`：0.24.0 与 0.25.0 渲染的 5 个组合完全一致。
- `just ci`：m2 的镜像构建在容器内下载依赖时失败一次（`failed to get serde`，构建第 151 秒），同一轮的 m3、m5 成功；单独重跑 m2 的全量档全部通过，判定为网络抖动。`just ci` 在这一步停下，其后的 `names`（两个版本）与 `version-info` 单独补跑，全部通过。
- 用户的 Docker 镜像在运行前后一致，没有残留的检查镜像或容器。
- 至此 §9 的验收项全部有证据；仍未验证的只有 workflow 在 GitHub 上的实际运行。

## V6.1 · 增补（参考 Quasar 后由用户认可的四项）

**做了什么**（提交 `e19ddc9`、`a567f9f`、`99b3fa0`、`393e44b`）

| 项                     | 改动                                                                                                                                                                                                                                      |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 可配置的 CORS          | api 的配置段新增 `server.cors_origins`（默认为空，即不启用）。文件里写数组，变量和命令行写逗号分隔的文本；每项须是 `http(s)://host[:port]`，`*` 只能单独使用。`tower_http::cors::CorsLayer` 放在 problem 中间件外侧，错误响应也带 CORS 头 |
| release 产物加 sha256  | `just package` 在压缩包旁生成 `.sha256`；release workflow 一并发布；全量档用 `shasum -a 256 -c` 校验                                                                                                                                      |
| 生成项目的 `AGENTS.md` | 一页：完成前的要求、各类代码放哪、守卫执行的规则、约定、常用命令                                                                                                                                                                          |
| mimalloc 可选 feature  | bin 的 `mimalloc` feature，默认关闭；README 写明用法                                                                                                                                                                                      |

**新增的配置键与依赖**（任务书 §8.5 要求先经用户同意；用户已认可）：`server.cors_origins`；tower-http 的 `cors` feature；`mimalloc`（MIT）。

**检查**

| 命令或操作                | 结果                                                                                                                                             |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `just commit`（每个提交） | 通过                                                                                                                                             |
| CORS 测试                 | 默认不启用；列出的 origin 在预检、成功响应和 problem 上都带头；未列出的不带；`*` 放行任意 origin；配置的数组与逗号文本两种写法，以及非法值的拒绝 |
| `check-config` 冒烟       | 环境变量给出的 origin 列表正确显示来源；`*` 与其他项混用以 78 退出                                                                               |
| mimalloc                  | `--all-features` 下 clippy 与 `cargo deny` 通过；带该 feature 构建的二进制含 mimalloc 符号                                                       |
| `just package`            | 生成的 `.sha256` 通过 `shasum -a 256 -c`                                                                                                         |
| `just full m2 m5`         | 27 步全部通过，含 sha256 校验、镜像构建与冒烟；用户的 Docker 镜像前后一致                                                                        |

## 补充 · 只支持一个 cargo-generate 版本

- 用户把系统里的 cargo-generate 升到 0.25.0 后，`just ci` 仍报"两个版本相同"：`compare` 要用最老支持版本（0.24.0）和最新版本分别渲染，`CG_MIN` 与 `CG_LATEST` 默认都指向系统里的同一个版本。
- 按用户的决定，改为只支持并只测试一个版本：`cargo_generate_version` 提到 `>=0.25.0`；删除 `compare`、`cg-versions` 与 `CG_MIN`/`CG_LATEST`；`names.sh` 用 `CG`（默认为系统里的 cargo-generate）；模板 CI 只安装 `CG_VERSION`（0.25.0），compare job 改为只跑 `just names`。这推翻了任务书沿用的 V5 D-14（`>=0.24.0` 并比对两个版本）。
- 以后升级时，同时改 `template/cargo-generate.toml` 与 `template-ci.yml` 中的版本。

## 补充 · pretty 日志与启动配置日志

参考 V3 生成的 fusion_server 截图后，按用户的决定改两处：

| 项                           | 改动                                                                                                                                                                                                                                  |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `log.format = "pretty"`      | 可选值，默认仍是 `auto`（终端单行文本，否则 JSON）。用 `tracing_subscriber` 的 pretty 格式：每个事件多行，带源码位置，供开发时阅读；日志文件仍是单行。不新增配置键或依赖                                                            |
| 启动日志 `configuration`     | 新增 `config.file`：实际读取的配置文件路径，没有就是 `none`（`Loaded` 新增 `file`，`table::file` 负责单行显示）。`overrides` 改为只列生效值与默认值不同的键并带来源；原先按来源过滤，`just run` 会列出 example.toml 的全部 15 个键 |

**检查**

| 命令或操作              | 结果                                                                                                                                         |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `just commit`           | 通过                                                                                                                                         |
| 新测试                  | 文件里写成默认值的键不列入 `overrides`；空配置文件仍记在 `config.file`；pretty 输出多行、带 `output.rs:` 位置与 span 字段，文件输出仍为一行 |
| 进程测试                | 改用非默认的 `--log-filter info,hyper=warn`（`info` 等于默认值，不会再出现），并断言 `config.file`                                         |
| `just run` 冒烟         | `overrides` 只剩命令行给出的 `log.format` 与 `server.http_addr` 两项                                                                       |

## 补充 · 访问日志耗时单位与 JSON 日志的 span

| 项 | 改动 |
| --- | --- |
| `http.server.request.duration` | 原为毫秒整数，快请求都记成 `0`，且与 OpenTelemetry 语义约定（秒）不符。改为秒的浮点数；计时改用 `tokio::time::Instant`，暂停时钟下的测试可断言精确值 |
| JSON 日志的 `span`/`spans` | 原先同时开启 `with_current_span` 与 `with_span_list`，同一个 span 在一行里写两遍。改为只保留 `spans`（全部打开的 span，由外到内），handler 自己再开 span 时 `request_id` 仍在 |

**检查**：`just commit` 通过；客户端放弃的请求在暂停时钟下记为 `0.1` 秒；嵌套 span 的 JSON 行只有 `spans`，`request` 在前；实跑的一行访问日志为 `"http.server.request.duration":0.000224916`，只有 `spans`。

## 补充 · 深度对比 V3/V4 后修复的缺陷与文档

对 V3/V4 做了六个方向的只读深度对比（错误与 HTTP、配置与 CLI、日志、运行时与 domain、模板工程、V4 阶段 C 缺陷清单）；以下 7 项按用户的决定逐个修复，每项一个提交。

| # | 问题 | 修复 | 验证 |
| - | ---- | ---- | ---- |
| 1 | 项目名 `mimalloc` 生成无法解析的 workspace（C-25 在 V6.1 再现：加依赖时没加进 hooks 的名单） | 名单加入 `mimalloc`；template-lint 从 `[workspace.dependencies]` 与 crate 后缀推导应拒绝的名字，与名单比对 | lint 在修复前恰好报出 `mimalloc`；`just names` 新增一项通过 |
| 2 | 每次启动都把日志文件归档；保留按文件数计，崩溃循环一天 14 次会删光更早的日志 | 当天写过的文件续写，更早的才归档 | 新测试在旧实现上失败（3 次重启产生 3 个归档） |
| 3 | `error.context` 只取最外层，`because`/`more_context` 包裹后内层 context 不进日志 | 沿链拼接各层 context（外层在前，`: ` 分隔）；problem 的 `detail` 仍只用最外层 | 单元测试覆盖 `because` 与 `more_context` |
| 4 | problem 中间件只改写无 content-type 的失败响应；直接用 axum 的 `Query`/`Json`/`Path` 会得到纯文本，与契约不符 | `text/plain` 的失败响应也改写，文本作 context；`ApiPath` 遇到 axum 判 500 的拒绝（参数个数不符）按本服务错误处理；`HTTPStatus` 不在 400–599 时按 500 | `problem/tests.rs` 用独立路由验证三种情形；JSON 失败响应（如 `/readyz`）保持不变 |
| 5 | 文档四处与实现不符 | 改正：监督器测试不再每场景 50 次；bin 的 `lib.rs` 含 `main`；closure 规则只对默认 feature 成立；`request_id` 只在放行 `svc_api` 错误的过滤下保留（含 `access.rs` 注释） | — |
| 6 | `check.sh` 的 `image_serves` 在 `git rev-parse`/`docker run` 失败时跳过清理，留下检查镜像；notices 目录从不删除 | 清理不再被跳过；scratch 目录放进工作目录；`trap` 在退出（含 INT/TERM）时删除副本及本次检查自己的容器与镜像 | 用 `FROM scratch` 的检查镜像复现：旧实现留下镜像，新实现删除；SIGTERM 中断后工作目录已删除；用户镜像前后一致 |
| 7 | probe 每步最多 5 秒，而镜像健康检查 3 秒就杀掉它；健康检查探 `/readyz`，排空或依赖故障时容器被判不健康 | probe 总时限 2 秒；健康检查改探 `/livez` | 新进程测试：对不应答的监听，probe 在 3 秒内以 `timed out` 失败（实测 2.006 秒）；`just full m2` 含镜像健康检查通过 |

**推翻或修正的既有记录**：容器健康检查从 `/readyz` 改为 `/livez`（V3 设计稿的主张，V4/V6 曾用 `/readyz`）。

## 补充 · 从 V3/V4 吸收的六项能力

用户认可了深度对比"三、值得吸收的能力"中的六项，逐项实现，每项一个提交。都没有新增配置键或依赖。

| 项 | 实现 | 测试 |
| -- | ---- | ---- |
| 前缀变量拼错时给提示 | 不含 `__` 的前缀变量仍然忽略（Kubernetes 会加），但若把 `_` 与 `__` 视为相同后正好拼出一个键（如 `APP_SERVER_HTTP_ADDR`），报错并给出应写的变量名；含 `__` 的未知键同样给提示（如 `APP_LOG__FILE_ENABLED`） | 配置单元测试；进程测试覆盖两种写法 |
| 探针请求不写 INFO 访问日志 | `/livez`、`/readyz` 的 `request finished` 记为 DEBUG（同一 target）；`log_at_level!` 增加 `target:` 形式；探针路径改为 `probes.rs` 中的常量，路由与访问日志共用 | 契约测试：两个探针为 DEBUG，其他请求为 INFO |
| 日志写入器 | 文件写入经 `BufWriter`（写线程每批之后 flush，已核对 tracing-appender 0.2.5 的 worker）；每秒至多检查一次文件是否还在，文件或目录被删后重建；轮转时文件已不在则直接新建；失败计数改为共享，由 `Guard` 在退出时报告；启动时删除上次留下的 `.gz.tmp`。`rolling.rs` 的归档部分移到 `rolling/archive.rs` | 目录被删后 1 秒内重建；目录变成文件时计数失败；孤立的 `.gz.tmp` 被删除、未完成的归档重新压缩 |
| `running` 带启动耗时 | `phase changed` 进入 `running` 时带 `startup.duration`（秒） | 暂停时钟下，较慢的服务 2 秒后就绪，记为 `2.0` |
| pre hook 提示删除残留目录 | 拒绝名字时追加"remove the directory 'x' that was created for it"；`--init` 不创建目录，不提示 | `just names`：两条路径各一项 |
| 模板 CI 的 lint job | 去掉 Rust 1.88 与 stable 的安装：`just lint` 只用 Python 与 shell | actionlint |
