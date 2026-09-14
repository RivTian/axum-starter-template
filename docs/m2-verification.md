# M2：单 runtime 服务切片验收

> 历史记录：本页描述 M2 验收时的切片。当前代码已推进到 M3，最新行为与验证见 `docs/m3-verification.md`。

- **日期**：2026-09-14。
- **阶段状态**：M2 的组件实现及本地验收完成；M3/M4/M5 尚未实施。
- **平台**：macOS arm64。本地验证不等于远端 Actions 已运行或其他平台已验收。
- **代码状态**：当前工作树增量，未创建 git 提交或推送；没有修改 `edge_dev` 或整体恢复备份。
- **边界**：这是一份真实可运行的单 runtime 切片，不是完整模板所有 55 项验收均已通过。

## 1. 已实现的链路

```text
CLI / 配置路径选择
  → TOML 文档解析 → 字符串环境展开 → 默认补全/规范化 → 组件与跨字段校验
  → app 唯一初始化 tracing
  → 同步 main 构造一个 multi-thread runtime
  → 安装持久 OS 信号接收器
  → 数据目录 → 持有 StorageOwner → 连接/迁移/自检
  → supervisor 登记 ticker → 目标 runtime 初始化并确认
  → supervisor 登记 HTTP → bind 并确认（此时尚不处理请求）
  → 唯一 Running 提交 → ticker / HTTP 开始工作
  → 信号或顶层任务退出
  → 撤销 readiness → 根取消 → 有界协作收割 → 必要时有界 abort 收割
  → 使用者排空有证据时限时关池
  → 离开 block_on → 同一截止上下文下同步销毁 runtime → 退出码
```

### 文件落点

仓库根目录：`/Users/riotian/Documents/code/axum-starter-template`。

| 能力 | 实际落点 |
| --- | --- |
| 同步入口、panic 边界和退出码 | `app/src/main.rs`、`app/src/rt.rs` |
| CLI、冷配置三段式加载 | `app/src/cli.rs`、`app/src/config/`、`config/service.toml` |
| 真实任务所有权、名字与退出分类 | `app/src/supervisor.rs` |
| 启动准备/确认/提交、监控和资源清理 | `app/src/boot.rs` |
| 首因、绝对期限和关闭结果 | `app/src/shutdown.rs` |
| 持久 OS 信号、唯一 tracing 初始化 | `app/src/signals.rs`、`app/src/telemetry.rs` |
| 生命周期读写能力与启动门 | `core/src/lifecycle.rs` |
| 示例 ticker | `worker/src/lib.rs` |
| HTTP、最小状态、错误与提取器契约 | `api/src/lib.rs`、`settings.rs`、`state.rs`、`response.rs`、`error.rs`、`extract.rs`、`handler/` |
| 单后端配置、门面与关闭所有者 | `storage/src/lib.rs` |

## 2. 当前运行契约

### 2.1 配置

优先级是 `--config <path>` → `<BINARY_NAME_UPPER>_CONFIG` → `./config/service.toml`。
主程序以 OsString 处理路径参数，组件参数不读取环境；SQLite 相对路径以配置文件目录为基准。

- 未知字段、非法类型、非法预算/范围、缺环境值、非 UTF-8、非普通文件均拒绝。
- TOML 先解析成值树，仅展开字符串值；环境内容不会变成 TOML 语法，不递归展开。
- 文件实际读取和环境展开分别有 64 KiB 上限。
- 实际存储参数必须是绝对文件路径，拒绝根目录/无文件名路径，避免把输入错误变成协调器 panic。
- 当前保存的是冷快照。**SIGHUP 记录 `reload_unavailable`，不应用文件变更**；配置 watch 是 M3。
- M2 未接受 `runtime.extra` 或任务 runtime 绑定字段，不能静默落回主 runtime。

当前开发默认值：request 2000ms、ready probe 500ms、ticker 1000ms；startup 10000ms、grace 5000ms、abort reap 1000ms、storage close 3000ms、runtime shutdown 1000ms。
这些是可编辑的资源策略，不是部署 SLA。监听 `127.0.0.1:0` 仅用于本地申请临时端口。

### 2.2 HTTP

| 路由 | 行为 |
| --- | --- |
| `/v1/service/health` | 200 成功信封；不查询数据库 |
| `/v1/service/ready` | 生命周期写端存活、Running 且有界 DB 探测成功才 200，否则 503；探测后复查状态 |
| `/v1/service/info` | 裸 JSON 服务名/版本，不暴露配置或数据库路径 |

根部与 `/v1` 内未知路径返回 JSON 404；不支持的方法返回 JSON 405 并保留 Allow；HEAD 不返回响应体。
提取器保留框架 400/413/415/422 等状态，5xx 泛化；请求处理超时返回同形 JSON 503。
请求日志仅记录 method、匹配的路由模板、status、latency，不记录完整 URI/query/authorization/body。

### 2.3 监督和关停

- 实际 future 直接由 JoinSet 持有；名字重复、停止后注册在 release 也拒绝。
- 保留意外正常返回、错误、panic、取消四类退出，任务先于同时可读的普通停止请求被观察。
- 启动确认不等于就绪：HTTP 绑定后仍在启动门后等待，只有 app 可以提交 Running。
- ack 关闭时不立即用“通道关闭”覆盖底层错误：在同一个启动 deadline 下等待真实任务退出，从而保留 bind 失败首因。
- 每阶段使用固定绝对边界与自身预算的较小值；abort 后仍有第二个收割期限，超时列出 unreaped，不进行无界 drain。
- 第二次停止请求升级 Forcing，不刷新时间线。
- 正常 HTTP 返回是排空证据；外层 abort/panic 不代表连接已结束。无证明或有 unreaped 时跳过正常关池路径并非零退出。
- 关闭超时/中断追加清理失败，不覆盖最初原因。最终日志区分 task exit summary、storage 结果、未收割名单和 elapsed_ms。
- 协调器 panic 由最外层窄边界转成失败退出；资源 guard 撤销 readiness/cancel/abort，外部期限和任务名单仍在同步 main 可见。

## 3. 实际验收

### 3.1 Rust 组件和装配测试

当前一次 workspace test 共 **51 项通过，0 failed，0 ignored**：

| 测试范围 | 数量 | 关键证明 |
| --- | ---: | --- |
| API | 9 | 系统端点、就绪二次检查、JSON/405/HEAD/提取失败、请求 timeout、启动门、正常/强制 HTTP 边界 |
| app 单元/装配 | 25 | 冷配置、首因与预算、真实监督器、同进程双实例、启动失败、强制/未收割/close timeout、协调器 unwind |
| app 保留的运行时机制测试 | 6 | 跨 runtime 机制、id/错误/panic、取消、blocking 限制；不代表 M4 配置拓扑已完成 |
| core | 3 | 非零周期、提前提交不丢失、取消、写端关闭不视为就绪 |
| storage | 5 | 空迁移/重启、失败不发布、初始化取消、多连接选项和 close、异常迁移元数据 |
| worker | 3 | 首周期/Skip、启动门/取消、确认通道异常 |

特别验证：

- 首个 bind 失败没有被随后关闭的启动确认通道覆盖，已启动 ticker 被回收且存储关闭。
- 人为不让出的任务穿过 graceful 和 abort 两个等待段后被记录为 unreaped；测试自身用释放 guard 回收线程，不污染 runner。
- HTTP 外层任务被取消后，即使 join 已返回，正常 storage close 分支仍被禁止；测试用存储门面再次 health 成功验证该分支确实未执行。
- 用受控 close future 验证 close timeout 不刷新期限、不覆盖原始错误。
- 两套 app 在同一进程/运行时独立启动和关闭，没有业务全局状态或全局测试 subscriber。

### 3.2 实际 debug/release 服务进程

`app/tests/check_service.py` 对 **debug 与 release 两个普通应用可执行文件各执行 13 组检查**，不是只运行测试 profile：

1. 三个系统端点与 JSON/404/405/HEAD 契约。
2. SIGHUP 保持冷配置，明确报告未实现热重载。
3. SIGTERM 正常退出，任务/存储/未收割结果可见。
4. 实际请求日志有正向记录，且 query/header 秘密不出现。
5. 配置路径基准、环境来源与 CLI 优先级。
6. SIGINT/Ctrl-C 正常退出。
7. 两个服务进程独立运行，停止一个不影响另一个。
8. 非法配置 fail-fast，不回显秘密。
9. FIFO 配置文件快速拒绝，不阻塞打开。
10. 真实端口占用失败，保留 bind 原因并关池。
11. 不可用数据目录 fail-fast。
12. 数据库初始化被外部 SQLite 锁保持时，停止信号仍能取消启动并清理，未提交服务。
13. 失败迁移元数据阻止下次启动且执行清理。

每个子进程都有外部时间上限和最终 kill/wait 清理；HTTP 测试禁用环境代理，不占固定端口。
每个实例使用自己的配置、数据目录、输出队列和临时工作目录。

### 3.3 release panic 与工程链

保留的 `m1-release-panic` fixture 已改为引用**实际的 M2 supervisor 源码**，不再用另一份 JoinSet 代替。
普通 release 构建必须保持 unwind；控制 panic 被监督器识别，sibling 被取消并 join，runtime 在同步入口关闭，进程按预期返回 1。
检查器返回成功，生产主程序没有 panic 注入开关或额外测试路由。

同时执行：

- `make check`：11 个 Python 工具测试、fmt、Clippy 全目标拒绝警告、workspace tests/build、release panic、迁移重编译、debug/release 进程检查。
- `make verify`：第二组名字、独立空 target 的完整生成检查。
- `crate_prefix=sqlx`：本地 sqlx-core 与 registry sqlx-core 同名的生成/锁文件/编译检查。
- 生成项目自己的 Makefile/CI 入口仍是 `make check`；两份 CI 分离，不复制模板 gen/verify 目标到项目。

## 4. 实施中发现并解决的问题

| 问题 | 处理 |
| --- | --- |
| TOML 1.x 的 Value::from_str 解析单值，不是整个配置文档 | 改用 `toml::from_str`；保留“合法样例与空文档等价”的正向测试，避免错误用例全绿却实际拒绝所有配置 |
| ack sender 关闭可先于 JoinSet 完成通知可见 | 等待实际任务退出，沿用原 startup deadline；绑定错误保留真实来源 |
| scoped 日志捕获在并行 API 单元测试中不稳定 | 日志契约放入独立实际应用进程，既断言有日志，也断言没有秘密；不串行化整个测试套件、不安装全局测试 subscriber |
| compact 日志会给字符串字段加引号 | 进程检查器按字段解析，不把 `storage="closed"` 误判成清理失败 |
| 根目录路径能进入 storage 参数却没有文件名 | 组件构造器要求绝对文件路径，拒绝无文件名路径，避免启动中的 expect panic |
| 只限配置文件大小不限制环境展开仍会放大内存 | 展开后字符串总量也限制为 64 KiB |
| 先 open 再判断文件类型会阻塞在普通 FIFO 上 | 打开前检查、打开后再检查 descriptor；保留对恶意路径替换和卡死文件系统的边界说明 |

## 5. 版本、证据与复现

沿用 Rust/MSRV 1.97.1、Tokio 1.53.1、Tokio-util 0.7.19、axum 0.8.9、sqlx 0.9.0、cargo-generate 0.24.0。
M2 新消费者固定 serde 1.0.229、serde_json 1.0.151、serde_path_to_error 0.1.20、toml 1.1.6、tracing-subscriber 0.3.23、tower-http 0.7.1、tower 0.5.3。
SQLite 仍只选择 sqlite-bundled；不使用 Any、PostgreSQL/MySQL 实现或新的业务迁移。

本次模板锁文件 SHA-256：

```text
029b527e16a88bd02622b201c781a9855bf5225ac01b19ccca7f075c198b8eeb
```

从模板根复现：

```sh
make check
make verify
python3 scripts/template.py check --name m2-collision --prefix sqlx
```

进入工具打印的生成项目后：

```sh
cargo run --locked
# 根据 service_started 的 listen_addr 访问三个端点，然后 Ctrl-C。
make check
```

## 6. 未完成项与保证边界

- M3：配置快照/publisher、单飞重载作业、冷热变更事务和 ticker watch 消费尚未实现。
- M4：配置 extra runtime、绑定解析、部分构造失败回滚及多 runtime 共用关闭预算尚未实现。
- M5：完整平台/模板产品化矩阵与远端 CI 验收尚未完成；本轮没有推送或运行远端 Actions。
- 无业务 schema；真实业务迁移的完整 checksum/DDL 中断/并发恢复契约仍须随实际用例验证。
- 普通文件 I/O、同步日志或不让出的代码若阻塞调度器，应用不能提供任意情况下的硬截止；需要外部进程管理者强制终止。
- force 路径不承诺业务 flush、在途请求无损或库内部所有线程立即停止；runtime shutdown 返回不被记录为逐线程 join 证明。
- 单请求 panic 可能只结束库内连接，不等于 HTTP 顶层面退出。

上述限制不被“51 个单测通过”或“进程返回 0”覆盖。完整架构的 55 项验收仍按阶段推进，当前只交付 M2 单 runtime 服务切片。
