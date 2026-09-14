# M3：配置热重载验收记录

> 历史记录：本页描述 M3 验收时的切片；当前代码已推进至 M4，最新状态见 `docs/m4-verification.md`。

- **日期**：2026-09-14。
- **结果**：M3 实现及本地验收完成；下一阶段为 M4。
- **平台**：macOS arm64；未推送或执行远端 Actions，不据此声称其他平台已验收。
- **边界**：仅 `ticker.interval_ms` 可热改。没有新增 HTTP 端点、业务字段、业务表、通用 Service/插件/任务工厂或 extra runtime。
- **工作树**：未创建 git 提交或推送；没有修改 `edge_dev`，没有整体恢复备份分支。

## 1. 当前实现

```text
startup LoadContext（固定文件路径 / 目录 / 环境快照 / 默认线程数）
  → 同一三段式管线
  → Config { boot: BootConfig, hot: HotConfig }

SIGHUP（仅 Running 接受）
  → 最多一个受持有的 spawn_blocking 作业
  → 再来请求：最多一个 pending 位
  → 完成回到 app 协调器
      ├── 超时 / 已关停：丢弃结果
      ├── I/O / 解析 / 校验错误：拒绝本次，保持旧快照
      ├── 任意冷字段变化：整批 requires_restart
      ├── 完全不变：no_change，不增代
      └── 仅热字段变化：一次 send_replace，代号 + 1
  → ticker 观察最新快照，重新构造 interval，记录已消费代号
```

生产代码落点（仓库根：`/Users/riotian/Documents/code/axum-starter-template`）：

| 文件 | 职责 |
| --- | --- |
| `core/src/config.rs` | HotConfig、ConfigSnapshot、非 Clone ConfigPublisher、只读 ConfigHandle |
| `app/src/config/mod.rs` | BootConfig/HotConfig 类型边界、冷变化报告、统一校验 |
| `app/src/config/load.rs` | 既有加载管线；M3 继续使用同一个捕获上下文 |
| `app/src/config/reload.rs` | 唯一提交点、单飞加载、合并请求、过期结果隔离、失败分类与关闭收割 |
| `app/src/main.rs` | 保留 Arc<LoadContext> 并注入固定加载函数，不在重载时重新读取环境/cwd/CPU 数 |
| `app/src/boot.rs` | 协调器选择控制事件/作业完成/超时；关停封提交，作业和顶层任务共用 grace/abort 期限 |
| `worker/src/lib.rs` | 最新快照消费、周期重建、取消优先、writer 关闭错误 |

没有让 blocking 作业持有 publisher，也没有把配置写端注入 API。

## 2. 固定的语义

### 2.1 快照与冷热边界

- 初始 generation 为 1；值和代号放在同一个不可变 Arc 快照里。
- 热值真正变化才增加代号；相同配置不通知，溢出明确拒绝，不能绕回 0。
- `send_replace` 在没有接收者时仍保存最新值；新读者可取得最新快照。
- `changed()` 使用 `borrow_and_update()`，不跨 await 持有 watch 借用；慢读者可以跳过中间代号，但值与代号不能混合。
- BootConfig 不通过 watch 发布；任何冷变化，包括和热变化混在同一文件中的变化，均整批拒绝。
- HTTP/Storage 的冷变化以组路径报告，其他当前冷字段用具体配置路径报告；不列值、不写回文件。
- 新的冷字段必须通过类型的完整比较；报告函数用穷尽解构约束新增字段的处理，不采用默认放行的字符串前缀分类。

### 2.2 单飞、超时和关闭

`lifecycle.reload_timeout_ms` 默认 2000ms，接受 (0, 60s]；它自身是冷字段。

- 从作业提交时开始计时，包含 blocking 池排队和协调器观察完成的时间。
- 最多一个在途作业，加一个合并后的用户请求。作业失败不会自动重试。
- 超时先撤销结果的发布资格，但保持原作业槽位；不会每超时一次就再创建一个堵塞线程。
- 旧作业真正结束后，至多再读取一次当时最新文件；迟到的旧结果不能发布。
- 每轮监控入口检查期限，避免重载事件不断就绪时压住超时处理。
- loader panic 即使发生在超时之后也升级为控制路径故障，不伪装成普通配置错误。
- 停止后不再提交，不再执行 pending 请求；已存在的作业进入原有 grace/abort 收割窗口，而不是再获得完整的新关闭预算。
- 已开始的 blocking 工作不能被 abort 强杀。未能确认收割则报告 `config_loader`，标记清理失败并非零退出，不虚报干净停止。

### 2.3 ticker 的实际消费

- 启动提交之后，重新取最新快照，等待完整首周期。
- 新代号被观察到时从“观察时刻 + 新周期”重新计时，丢弃旧 deadline，不补打一拍。
- 周期变化时重建 interval。**`reset_at` 不改变 interval 的后续 period**，只改下一拍会造成“首拍新周期、后面还是旧周期”的错误。
- 相同或已消费代号不重复重置，避免启动时 current() 与随后 changed() 重复读同一版本。
- 保持 Skip；取消优先于配置变化和 tick。writer 在非关闭状态消失会让 worker 返回错误，进入顶层监督。
- 发布与消费分别记录 `config_reload` 和 `ticker_config_applied`；tick 自带 `config_generation`。它们可能跨线程交错，不是配置 ACK 注册系统。

## 3. 验收结果

### 3.1 Rust 测试

当前一次 workspace test：**72 项通过，0 failed，0 ignored**。

| 范围 | 数量 | 内容 |
| --- | ---: | --- |
| API | 9 | 保留系统端点、就绪、错误/提取器、启动门和 HTTP 关闭契约 |
| app 单元/装配 | 40 | 保留 M2 生命周期；增加单飞、冷热拒绝、过期、晚到结果、loader panic、慢加载期间 HTTP/Stop 响应及关闭集成 |
| 运行时机制 | 6 | 保留跨 runtime/取消/阻塞限制的 SDK 机制测试，不将其当作 M4 已实现 |
| core | 7 | 快照一致、无读者发布、晚订阅、跳代、no-change、关闭与代号溢出，以及原有生命周期/值类型测试 |
| storage | 5 | 保留空迁移、初始化/关闭所有权、多连接与异常迁移元数据 |
| worker | 5 | 首拍/Skip/启动门、真正的新周期连续两拍、重复代号、writer 关闭与取消 |

关键故障注入不是 sleep 猜测后台是否开始：使用 oneshot 和可释放门闩确认 blocking 作业已进入。
超时相关检查使用明确 deadline；测试自己最终释放/回收故意阻塞的工作，避免污染 runner。

已证明的关键风险：

1. 100 次重载请求合并为一位 pending；超时后再请求也不增加实际在途调用数。前一作业结束后总共仅运行一次合并后的补读。
2. 没有先轮询 timeout 分支时，协调器在截止时间后才处理的完成结果仍被拒绝。
3. 超时后的 panic 仍是 ReloadFailure，而不是 discarded_expired。
4. 关停后到达的合法配置不增代，pending 不启动；已经开始的阻塞工作仍保留所有权直到 join。
5. 慢加载不会阻塞 HTTP readiness 或停止输入；超过关闭宽限的作业被记作 unreaped，而不是无限等待。
6. grace 内完成的加载被丢弃，可以正常关池、退出 0；并非只要关停时有重载就必须强制退出。
7. loader panic 使真实协调器撤销服务并关闭，而普通解析错误不会终止服务。
8. 冷热混合输入保留全部旧值；所有当前冷字段均有类型化拒绝/报告测试。

### 3.2 实际 debug/release 进程

`app/tests/check_service.py` 对普通 **debug 和 release 应用各执行 19 组检查**。
新增及更新的重载场景包括：

- 无变化保持 generation 1。
- 无效文件拒绝且保留运行值，不改写操作者的磁盘文件。
- 只改 ticker 周期，发布 generation 2，观察消费者及实际 tick 使用新值。
- 再次加载相同值不增代。
- 冷热混合修改整批 requires_restart，ticker 继续使用旧周期和旧代号。
- 仅冷字段修改仍拒绝。
- 再次热改到另一个周期，generation 3 的连续 tick 使用该值。
- SIGHUP 与 SIGTERM 竞态：停止被协调器观察后不再出现 published；停止之前已提交的版本是合法的。

同时保留系统端点/错误、日志脱敏、配置路径与 CLI 优先级、独立进程、SIGINT/SIGTERM、启动失败/取消与迁移错误检查。
日志检查器使用共同检查点和历史记录，能处理“消费日志先于发布日志打印”的合法调度，不会吞掉提前到达的证据。

### 3.3 工程链

执行默认 `make check`、独立空 target 的 `make verify`、`crate_prefix=sqlx` 同名包生成，以及生成项目自己的 `make check`。
门禁包含 fmt、Clippy 全目标拒绝警告、workspace test/build、实际监督器 release panic、迁移增改删重编译及上述进程检查。
Python 维护工具仍有 11 项测试，并在普通与 `python -O` 模式验证。

M3 不增加第三方依赖，沿用 M2 的锁定版本。模板锁文件 SHA-256 保持：

```text
029b527e16a88bd02622b201c781a9855bf5225ac01b19ccca7f075c198b8eeb
```

## 4. 本阶段发现与修正

| 问题 | 修正与证据 |
| --- | --- |
| 仅 reset_at 导致后续仍按旧 period tick | 重建 interval；虚拟时间检查连续两拍，原错误被该测试实际捕获 |
| current() 可能已读到随后 changed() 返回的同一代 | 比较已消费代号，相同代号不重置，不重复声称消费 |
| 超时可能先于协调器处理完成结果 | complete 再检查原 deadline，不能因为完成分支先被选中就绕过时限 |
| 接收者不存在时普通 send 可能无法保留值 | 用 send_replace，并测试零读者及晚订阅 |
| blocking 超时后立即清空槽会制造无界堆积 | 槽跟真实 join 绑定；超时只撤销发布资格 |
| 消费日志可以先于 publisher 日志出现 | 进程检查按共同检查点查询保留的历史，不假定跨线程日志顺序 |

## 5. 使用与复现

从模板根执行：

```sh
make check
make verify
python3 scripts/template.py check --name m3-collision --prefix sqlx
```

进入生成目录启动服务后，修改原配置文件中的 `ticker.interval_ms`，再发送 SIGHUP：

```sh
cargo run --locked
# 另一个终端，使用实际服务 PID：
kill -HUP <service-pid>
```

观察 `config_reload` 的结果和 generation，再看 ticker 的 `config_generation`；冷修改需重启，错误文件由操作者修复。
路径、环境值和默认 worker 数使用进程启动时捕获的上下文，不能靠改外部环境或切换 cwd 改变重载来源。

## 6. 尚未完成及限制

- M4 的可配置 extra runtime、任务绑定和多 runtime 构建/关闭矩阵未实施；这些配置字段继续明确拒绝，不能静默回落。
- M5 的完整产品化/平台/远端 CI 验收未完成；架构文档的 55 项总体矩阵不因此全部标绿。
- 普通文件或底层阻塞代码卡死时，Tokio abort 不能杀线程；应用只能有界等待并报告不完整退出，硬截止依赖外部进程管理。
- 没有逐消费者 ACK；watch 是最新状态，不是逐条必须投递的事件日志。
- 不会回写配置，不重启失败任务，不引入业务迁移、表、指标或全局业务状态。
- 本阶段未提交或推送 Git，未执行远端 Actions。
