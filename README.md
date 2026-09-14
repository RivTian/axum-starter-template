# Rust Web 服务模板

五 crate 单向分层、受监督任务面、SQLite 单后端的 Rust Web 服务起点。
默认一个多线程 Tokio runtime，可将 ticker / HTTP 冷绑定到独立 runtime；只有 ticker 周期允许热重载。
不是框架，不携带业务表、插件系统或部署目录约定。

本地验证基线及平台限制见 `docs/verification.md`；架构约束见 `docs/architecture.md`。
维护历史只保留在仓库文档，不成为生成项目的注释、文件名、测试标签或依赖。

## 最短闭环

前置：Rust `1.97.1`（含 rustfmt/clippy）、cargo-generate `0.24.0`、Python **3.11+**、Make、C 编译器。
工具链和 Rust 依赖均固定，不使用 `latest` 安装策略。

```sh
cargo install cargo-generate --version 0.24.0 --locked
make gen GEN_NAME=my-service GEN_PREFIX=myapp
```

工具会打印唯一的 `GENERATED_PROJECT` 绝对路径。进入该目录后：

```sh
make check
cargo run --locked
```

看到 `event="service_started" listen_addr=...` 后，用实际地址访问三个系统端点：
`/v1/service/health`、`/v1/service/ready`、`/v1/service/info`，最后用 Ctrl-C 停止。
模板根 manifest 含生成占位符，**不能在模板根直接 cargo build/run/test**。
如果系统 `python3` 仍是旧版本，通过 `make check PYTHON=/absolute/path/to/python3` 显式选择 3.11+；不是要求修改系统 Python。

应用日志保持 compact 单行结构化格式：stdout 为终端时自动彩色，文件/管道或非空 `NO_COLOR` / `TERM=dumb` 时保持纯文本。
模板更新不会回写已有生成项目。重新体验需要再次 `make gen`，不要覆盖已经开始开发的服务。

## 维护门禁

```sh
make check         # 工具单测 + 默认生成项目完整 check + 四组名称矩阵 + 并行/非法名称探针
make matrix        # 单独执行 short/long/hyphen/依赖撞词矩阵；真实 fmt/clippy/test/build/启动
make verify        # 第二组身份、空 target，从零编译依赖并运行生成项目完整 check
make fmt           # 树外生成并格式化，再按原样字节回写 Rust；冲突时拒绝写入
make lock          # 仅明确调整依赖时运行；结构化映射 workspace 身份
make tooling-test
```

`check` 和 `verify` **直接调用生成项目自己的 Makefile**，不是另写一套近似门禁来冒充生成项目验证。
每个完整 gate 包含 debug/release × 五布局进程检查、PTY/管道/文件日志、真实 release panic、真实迁移增改删重编译与重新启动。
报告输出为 `VERIFICATION_REPORT`、`MATRIX_REPORT`；报告仅证明其中记录的平台/工具版本，不代表远端 Actions 已运行。
维护入口要求原始项目名为小写 kebab-case（最多 64 字符），包名前缀最多 32 字符。直接使用 cargo-generate 时，工具可能先规范化名字；pre hook 校验最终名字，`--force` 保留非法原名时仍会拒绝。
名称矩阵校验五 crate 清单、normal/dev/build 边、固定别名、实际二进制与环境前缀、单后端 feature 闭包和生成残留。

## 目录安全与复用边界

每次生成都在树外创建唯一目录，无可变的“最后生成项目”全局指针，不覆盖上次结果。
维护入口显式传入 `--no-workspace`，即便生成根位于另一个 Cargo workspace 内，也不改写其成员列表。
`make clean DIR=<工具打印的受管项目绝对路径>` 仅删除路径和所有权标识匹配的受管目录；不接受符号链接授权删除。
默认生成根为 `~/.cache/axum-starter-template/workspaces`，受管父目录统一用 `gen-` 前缀；报告名为 `verification.json` / `matrix.json`。
仅复用 Cargo 管理的依赖/构建缓存；`verify` 不复用 target。新默认位置不会搬迁、覆盖或删除旧生成工程。
需要清理历史受管目录时，显式传入其原 `GEN_ROOT`；旧目录仍须通过原有的路径/所有权校验。
`fmt` / `lock` 通过同一写锁串行化；回写前验证源码快照，`fmt` 会先检查全部文件再写入，不把格式化结果反向替换成模板。
编辑器不参与写锁；不要在 fmt/lock 回写期间同时编辑相同文件。`make -j check` 内部仍按顺序执行各阶段。

真实包名展开为 `<prefix>-core` 等；Rust 固定使用 `service_core` 等依赖别名，长名称不影响源码格式。
锁文件只映射 workspace 身份；包括 `sqlx-core` 同名情况在内，第三方包名、版本、source、校验和不被全局替换。
模板 CI 与生成项目 CI 分离。后者经 post hook 从 `project-ci` 移入 `.github`，不是把模板的 gen/verify 作业复制出去。
`.genignore` 负责递归过滤 Python 缓存/OS 元数据，`template.ignore` 只用于字面路径。

工程复用来源和决策历史集中保存在架构文档；生成项目不继承这些历史背景。
配置默认样例仍作为外部开发配置交付；不自动落盘缺失配置，也不以可执行文件位置推导 data/安装根。
平台范围、失败边界和可重复证据见 `docs/verification.md` 与 `docs/acceptance.md`。

## 生成内容边界

`make gen` 本身就执行输出审计：随项目交付的源码、注释、测试、配置与 README 不得引用实施阶段或未交付的设计文档。
非渲染文件按字节检查，渲染文件检查其原始模板，避免误伤用户自己选择的合法项目名称。
历史报告留在模板仓库，不随生成项目复制；注释应就地解释当前代码的约束、原因和失败边界。

release panic 夹具保留在 `app/tests/fixtures`，而非 `app/examples`。它是生成项目自带的回归测试，不能用删除夹具来规避历史命名问题。
Cargo 的 named example target 仅作为真实 release 的编译入口；由 `make release-check` 检查其预期失败退出，不是额外服务入口。
