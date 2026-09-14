# M4：可选 runtime 绑定验收记录

- **日期**：2026-09-14。
- **结果**：M4 实现与本地验收完成；下一阶段为 M5。
- **平台**：macOS arm64。本轮未推送、未执行远端 Actions，不据此声称其他平台已验收。
- **范围**：可选拓扑、线程预算、任务绑定/监督、跨 runtime I/O/存储、部分构建失败清理和同步有界销毁。
- **未增加**：业务字段/表/端点、Service trait、插件系统、Executor trait、运行时热迁移或第二个监督器。

## 1. 当前实现

| 部分 | 实际落点与职责 |
| --- | --- |
| 配置与边界 | `app/src/config/runtime.rs`：Binding、RuntimeSpec、名字/引用/预算校验；它们均属于 BootConfig 冷状态 |
| runtime 所有权 | `app/src/rt.rs`：同步 main 独占 RuntimeSet；只有多线程调度器，包括 single-worker extra |
| 执行位置解析 | Executors 仅供 app 启动时解析 Handle；不进入 AppState 或 worker，不增加每请求/每 tick 查找 |
| 顶层监督 | `app/src/supervisor.rs` 仍以一个 JoinSet 直接持有实际 future；TaskExit 保存任务名和实际 runtime 名 |
| 装配 | `app/src/boot.rs` 启动前解析两个目标，在各自 Handle 上登记 ticker / HTTP；共享存储和重载作业保持在主 runtime |
| 关闭 | 排空自有任务及重载作业 → 有证明时关池 → extra 逆序关闭 → main 最后；全部使用同一剩余 deadline |

配置示例（不是默认配置）：

```toml
[ticker]
interval_ms = 1000
runtime = "compute"

[runtime.extra.compute]
worker_threads = 1
max_blocking_threads = 2
```

HTTP 同样支持 `http.runtime`。省略或写 `main` 都归一到主 runtime；允许两面共享一个 extra，也允许分别绑定两个 extra。
不认识的名字绝不回落到 main。

### 配置校验

- extra 名称最多 32 字节，小写 kebab-case；`main` 是保留名。
- 每个 extra 必须显式给出 worker_threads 和 max_blocking_threads，且均为正数。
- 最多 8 个 extra；worker 总数和 blocking 上限总和分别不超过 256。
- 当前只有 ticker / HTTP 两个面，因此没有实际绑定的定义会被拒绝，不能预建空闲执行池。
- 在文件加载和 RuntimeSet::build 入口都校验完整拓扑；程序式构造也不能绕过后再启动线程。
- 拓扑及绑定是冷字段；合法的拓扑变更和热周期混改仍整批 requires_restart，不创建新 runtime、不迁移任务。

## 2. 默认路径与隔离边界

未配置 extra 时：

- RuntimeSet 只构建主 runtime。
- extra owner Vec 和 executor Handle Vec 的 capacity 都为 0；main 的任务标签使用借用字符串，不为配置名称分配堆对象。
- ticker / HTTP 直接登记到主 runtime，没有 watcher-of-JoinHandle、额外转发任务或通道。
- 配置、元数据类型仍有少量静态源码/字段成本；不宣传为逐字节或逐指令零成本。

extra 隔离的是调度队列和线程预算，不是 CPU 时间、内存或进程故障的硬隔离。
测试故意占满 extra 的唯一 worker 后，主 runtime 上的真实 HTTP readiness 仍可响应；这证明执行器隔离，并不证明任意 CPU 负载下的服务 SLA。

HTTP 的 socket 在 API future 被目标 runtime 轮询后绑定，ticker timer 同理。
启动日志 `task_polled` 记录实际线程上下文，退出 summary 记录 runtime 名称；连接池不是每个 runtime 各开一份。

## 3. 构造失败与关闭契约

### 3.1 部分构造失败

按照 main、extra 名字顺序构建。第 k 个 runtime 构建失败时：

1. 保留失败 runtime 名和原始 I/O 错误。
2. 对已经构造的 extra 逆序关闭，main 最后关闭。
3. 只给这一轮清理一个共同截止时间，不逐个发完整新预算。
4. 报告清理预算是否耗尽；不将原始错误覆盖为最后一次 cleanup 信息。

测试在第三个 runtime 故意返回构造错误，并用真实任务 Drop 收据确认已经构造的 compute/main 按顺序收尾。
无效的程序式拓扑还验证了 builder 调用次数为 0。

### 3.2 共享关闭预算

正常应用先完成异步任务/存储清理，再退出 block_on 进行同步 runtime 销毁。
RuntimeSet 保留首次截止时间，后续调用和 Drop 兜底不能刷新它。

测试让两个 extra 和主 runtime 的 blocking 工作都保持运行，再发起有界关闭：

- 调用顺序为 network、compute、main。
- 第一个 runtime 消耗剩余等待预算后，后两个收到的是 0，而不是另一个完整预算。
- 测试最后显式释放阻塞工作并确认其完成，避免用测试本身制造泄漏。

`shutdown_timeout` 限制等待，不强杀仍在执行的阻塞线程；日志保持 `runtime_shutdown_returned`，不冒充逐线程 join 证明。
HTTP force 后没有排空证明时，仍遵循 M2/M3 的失败关闭路径，不因换了 runtime 就声称正常关池。

## 4. 本地验证结果

### Rust 测试

当前一次 workspace test 共 **82 项通过，0 failed，0 ignored**：

| 范围 | 数量 |
| --- | ---: |
| API | 9 |
| app 单元、配置、装配、RuntimeSet | 50 |
| 保留的 runtime 机制测试 | 6 |
| core | 7 |
| storage | 5 |
| worker | 5 |

M4 新增或增强的证明包括：默认无 extra 分配、single-worker extra 实际运行、任务 runtime 标签、无效拓扑副作用前拒绝、部分构造失败清理、统一关闭预算、首次 deadline 不刷新、extra 饱和时主 HTTP 响应、冷拓扑混合变更拒绝及名字/数量限制。

### 真实进程矩阵

普通 debug 和 release 应用分别执行以下五种布局：

| 布局 | ticker | HTTP |
| --- | --- | --- |
| main | main | main |
| worker | compute | main |
| http | main | network |
| shared | isolated | isolated |
| split | compute | network |

**每个 profile × 布局组合执行 23 组检查**（每份生成项目合计 10 组合）。
除了此前端点、配置、热重载、信号、初始化取消和迁移错误用例，还校验：

- 实际任务线程与配置 runtime 对应，runtime_count 正确。
- 未知绑定、闲置定义、保留名和总预算越界在 runtime_built 之前拒绝。
- 热重载不改变既有 runtime 拓扑；合法冷热混改整批拒绝，服务仍可探活。
- 存储关闭收据出现在 runtime teardown 之前。
- 每个 runtime 恰好有一条关闭等待结果，extra 逆序，main 最后。

release panic fixture 也已在 extra runtime 触发受控 panic，使用实际 Supervisor 核对 runtime 名，取消并 join sibling 后返回预期非零结果。
生产主程序没有故障注入开关或测试路由。

### 工程门禁

执行默认 `make check`、独立空 target 的 `make verify`、`crate_prefix=sqlx` 同名包生成检查，以及生成项目自身的 `make check`。
门禁包含 fmt、Clippy 全目标拒绝警告、上述 Rust/进程矩阵、release panic 与迁移增改删重编译验证。
维护工具的 11 项 Python 单测保留，并使用 `python -O` 复验。

## 5. 本阶段修复的工程细节

本地 Python 语法检查生成的 `app/tests/__pycache__` 被复制到生成结果，导致文本门禁遇到二进制字节码。
核对 cargo-generate 0.24.0 源码后确认：`template.ignore` 接受字面路径，不展开 glob。

已新增 `.genignore` 承担递归缓存/OS 元数据过滤，并在结构门禁中拒绝残留 __pycache__。
不是简单删除本机缓存后宣称模板生成已修复，也没有吞掉 Unicode 错误绕过检查。

## 6. 依赖、复现和限制

M4 未增加第三方依赖。继续使用 Rust/MSRV 1.97.1、cargo-generate 0.24.0 与现有锁定依赖，仍只编译 SQLite 后端。
模板锁文件 SHA-256 不变：

```text
029b527e16a88bd02622b201c781a9855bf5225ac01b19ccca7f075c198b8eeb
```

从模板根执行：

```sh
make check
make verify
python3 scripts/template.py check --name m4-collision --prefix sqlx
```

进入生成项目后，可单独检查某个组合：

```sh
python3 app/tests/check_service.py --profile release --topology split
```

当前仍未完成 M5 的完整产品化/平台/远端 CI 验收。架构文档的 55 项总体验收不是因为本页有 82 个单测就自动全部通过。
没有业务 schema、业务全局状态、具体部署端口、交叉编译目标、容器链或 CPU 配额保证；任意不可中断代码的硬截止仍由外部进程管理者负责。
本轮未提交、推送或执行远端 Actions，`edge_dev` 保持未修改。
