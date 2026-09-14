# M5：模板产品化验收记录

- **日期**：2026-09-14。
- **结果**：M5 实现与本地产品化门禁通过；不等同于已推送/发布或远端 CI 已通过。
- **实际平台**：macOS 26.6.2，arm64。Linux/macOS workflow 已定义并静态校验；未触发远端 Actions，Windows 未验收。
- **工具**：Rust/MSRV 1.97.1；cargo-generate 0.24.0；Python 3.14.7（脚本要求 3.11+）；actionlint 1.7.7 仅用于本地 workflow 静态校验。
- **范围**：GEN 全矩阵、生成项目真实 Makefile、空 target、迁移/协议负向证据、文档与平台边界。55 个验收 ID 的证据逐项见 [acceptance.md](acceptance.md)。

## 1. 实施结果

### 生成与安全

- `make check` 执行工具测试、默认项目完整 gate、四组名称矩阵及生成安全探针；`make matrix` 可单独重跑名称矩阵。
- 每组独占树外源码目录，复用的仅是 Cargo 构建缓存；无“last generated”指针，也不覆盖已生成工程。
- `--no-workspace` 防止 cargo-generate 自动把新项目添加到另一份父 workspace 的 Cargo.toml。实际嵌套 workspace 探针确认父文件字节未变。
- 同名请求并行生成两个不同目录，检查各自结构；清理其中一个后，另一个的用户修改 sentinel 保持不变。
- clean 拒绝非法路径/所有权不匹配/符号链接标识；fmt/lock 用同一写锁串行，fmt 先验证全部源码快照再按字节回写。
- 新增失败输出保留测试：捕获 stdout 的子命令失败时仍显示诊断，不只留下一个退出码。

### 名称与分层

| 案例 | project-name | crate_prefix | 实际执行 |
| --- | --- | --- | --- |
| 默认完整工程 | `example-service` | `example` | 完整项目 check + 嵌入迁移探针 |
| 最短 | `a` | `x` | fmt / Clippy / 91 Rust tests / build / 身份启动探针 |
| 长度边界 | `long-` 加 59 个 `n`（64 字符） | 32 个 `p` | 同上，源码格式不随名字长度改变 |
| 连字符且前缀不同 | `billing-edge-demo` | `svc` | 同上，二进制与环境前缀按项目名派生 |
| registry 撞词 | `registry-name-collision` | `sqlx` | 同上；本地 sqlx-core 与 registry sqlx-core 共存，不误改/误报 |
| 独立 clean-room | `clean-room-service` | `verifyx` | 独立空 target，从依赖起完整编译 debug/release，再运行完整项目 check |

metadata 检查五 crate 清单、normal/dev/build 的单向边及路径、固定别名、subscriber 生产依赖仅属于 app、SQLx 仅属于 storage。
实际编译 feature/tree 只有 SQLite；不把锁文件中未启用的后端解析项当作已编译依赖。
渲染白名单之外的 Rust/CI 按字节验证；生成结果无 hooks、模板工具/报告、`.project` 中间件、旧业务 crate/部署链或 Python 字节码。

直接 cargo-generate 默认会先规范化项目名（例如 `Bad_Name` → `bad-name`）。本轮首次负例把这一行为误当成应拒绝，检查因此失败；核对 0.24.0 CLI/源码后修正为：
维护入口检查原始名称，pre hook 检查最终名称；使用仅保留名称、**不授权覆盖**的 `--force` 检查非法原名拒绝。三个负例最终均按预期非零退出。

## 2. 新增运行时证据

生产变化保持很小：默认配置路径/优先级纯函数收敛到 app 的 config 模块；main 仍捕获进程输入，启动日志只增加安全的 `config_source` 类别。
StorageOwner 内部增加私有 `initialize_with`，测试与生产使用同一 connect → migrate → health 路径；生产入口始终传静态内嵌迁移集，没有开放 SQLx/migrator API。

### 真实迁移

测试专用临时 schema 验证：

- 成功版本重跑不重复应用。
- checksum mismatch、缺失版本、dirty、非法 SQL 均失败，不修复/覆盖元数据、不发布 facade。
- 两个 owner 同时初始化，共享文件的获胜结果能在重启后重新验证；允许竞争者 fail-fast，不偷偷重试。
- 显式持有 SQLite 独占锁时，初始化等待在预算内失败，owner 可关闭；解除锁后重启成功；关池后的 health 明确失败。
- SQLite progress handler 确认长 SQL 已开始，再中断事务 fixture；关闭、重开并重新执行迁移成功。不是对非事务 SQL、进程掉电或任意业务恢复的保证。

维护探针只修改本次带标识的生成目录。每次变更先确认未变源码为 fresh，再验证新增/修改/删除会重新编译；随后启动**实际服务二进制**检查：
`add-embedded`、`restart-does-not-reapply`、`modify-checksum-rejected`、`invalid-sql-fail-fast`、`remove-missing-version-rejected`、`remove-embedded-empty`。
探针最后删除临时 SQL 并重新构建正常服务；模板与生成项目的生产迁移目录都不留下业务表定义。

### HTTP 与关停

- 每个 debug/release × 五布局进程组合由原来的 23 个场景扩展到 **25 个**：新增未完成 header 下的有界退出与客户端断开后继续服务。
- 慢连接退出若正常，必须有真实 graceful/关池记录；若强制，必须记录 `skipped_unproven`，不能假报正常关池。
- API 仅测试路由的 handler panic 结束对应连接，但同一个 HTTP face 仍能处理下一请求；没有给生产主程序增加 panic 开关或测试路由。
- 既有 release panic fixture 继续执行，仍使用实际 TaskSupervisor，不用 test profile 替代真实 release 的 unwind/退出证明。

## 3. 最终本地验证

| 验证 | 结果 |
| --- | --- |
| `make -j check` | 通过；阶段按序，默认完整项目 + 四组名称 + 并行/父 workspace/非法名探针 |
| 每组 workspace Rust tests | **91 通过**；默认、四组名字与 clean-room 各运行一遍，不把重复运行累加成独立用例数量 |
| Python 工具测试 | 最后单独复验 **19 通过**，普通解释器与 `python -O` 均通过；完整生成 gate 后补充的诊断保留分支也单独验证 |
| 默认完整项目实际服务 | debug/release × 5 布局 × 25 场景 = **250 场景通过** |
| clean-room 实际服务 | 同样 **250 场景通过**；target 在开始时不存在，非共享增量缓存 |
| 终端日志 | 每个完整 gate 各 **12 场景通过**：PTY 自动样式、NO_COLOR（含空值）、TERM=dumb、管道、文件 |
| 实际内嵌迁移变更 | 默认与 clean-room 各 6 场景通过，包含新二进制和重新启动 |
| 生成项目 Makefile | 上述完整 gate 直接调用它，含 fmt/Clippy/test/build/release/logging/service，不是模板自己模拟相似步骤 |
| Workflow 静态验证 | 两份 workflow 经 actionlint 1.7.7 校验退出 0；未安装 shellcheck，因此此轮禁用其外部 shellcheck 集成 |
| 依赖 / 生产目录 | 无新第三方 Rust 依赖，无生产业务迁移，锁文件保持不变 |

最终原始报告（本机受管目录）：

```text
/Users/riotian/.cache/axum-starter-template/m1/m1-example-service-zltpfchm/m5-result.json
/Users/riotian/.cache/axum-starter-template/m1/m1-parallel-probe-_08nffaf/m5-matrix.json
/Users/riotian/.cache/axum-starter-template/m1/m1-clean-room-service-k1m_d9w9/m5-result.json
```

最终日志：`/tmp/axum-m5-final-check.log`、`/tmp/axum-m5-final-verify.log`、`/tmp/axum-m5-final-tooling.log`、`/tmp/axum-m5-final-tooling-opt.log`、`/tmp/axum-m5-actionlint.log`。
原始报告随受管目录/本机临时目录清理可能删除；可用同一组 make 命令重新生成证据。

模板 Cargo.lock SHA-256：

```text
029b527e16a88bd02622b201c781a9855bf5225ac01b19ccca7f075c198b8eeb
```

## 4. CI、文档与未完成的平台事项

- 模板/项目 workflow 分离，权限只有 contents:read，含去旧并发组、超时与 Linux/macOS 作业。
- checkout 固定 `11d5960a326750d5838078e36cf38b85af677262`（v4.4.0），cache 固定 `0057852bfaa89a56745cba8c7296529d2fc39830`（v4）；本轮与官方 git tag ref 核对。
- 缓存键包含 OS/架构、Rust toolchain、锁文件；模板还包含 generator 配置。空 target 验证不恢复共享 target。
- 新 README 包含生成→检查→启动→三个端点→停止，以及第一条迁移、ticker 删除、配置/data 路径、热重载、runtime、日志和错误边界。
- **未推送、未运行远端 Actions，不能称 Linux、macos-15 runner 或 Windows 已验收。** 当前可证实的运行环境仅为上面列出的 macOS arm64。
- 未执行生产负载、网络文件系统挂死、断电/任意业务 schema 恢复测试，不把模板超时预算宣传为生产 SLA。
- 没有创建 git 提交，也未修改 `edge_dev` 或整体恢复备份。现有生成项目未被覆盖。

## 5. 后续操作

1. 使用 `make gen` 创建新实例体验；已开始业务开发的生成项目按差异自行迁移，模板不会自动回写。
2. 提交/推送需要独立操作；之后观察两份 workflow 的真实 runner 结果，再决定扩展平台支持声明。
3. 新增业务迁移或移除 ticker 时，替换对应示例断言，保留监督、错误边界与关停的真实进程回归。
