# {{crate_prefix}}-runtime

任务监督：supervisor、任务面契约、五类退出、关停预算与报告。

**不依赖 tracing、不拥有 runtime**：退出记录走 `broadcast` 通道，打日志是装配层的事；
spawn 目标由装配层通过 `RuntimeSet` 提供（这里只拿 `Handle` 用）。

## 边界

- 任务面**返回 future**，不写 spawn；启动、重启、关停的生命周期全部由 supervisor 负责。
- 不做业务：这里只有"怎么把任务跑对"的机制，没有任何领域逻辑。
- 不认识配置：退避参数通过 `SharedBackoff` 注入（半热段由装配层在重载时写它）。

## 目录

- `src/spec.rs`：`TaskSpec` / `TaskContext` / `TaskFuture` / `RestartPolicy` / `Backoff`（含 `SharedBackoff`）。
- `src/exit.rs`：`ExitCause`（谁的意图）+ `TaskOutcome`（future 的结局）+ `ExitRecord`。
- `src/id.rs`：`TaskKey` / `RuntimeId` / `RuntimeSet`。
- `src/shutdown.rs`：相位机（`StopSignal` / `StopToken`）、`ShutdownBudget`、`ShutdownReport`。
- `src/supervisor.rs`：注册 / 启动 / 运行 / 重启 / 关停序列。
- `tests/supervisor.rs`：五类退出、单化身、重启边界、空 supervisor、runtime 归属、广播通道。
- `tests/shutdown.rs`：分层预算、二次信号加速、诚实上报（`still_running`）、资源逆序关闭。

## 关键决策

- **注册 ≠ 运行**：`register` 只挂账，`start` 才校验 runtime 并 spawn（校验失败不留半个进程在跑），
  `run` 是提交点之后的运行期与关停。
- **`cause` 与 `outcome` 分开**：`cause` 回答"这次退出是谁的意图"（任务自己 vs supervisor 要求停），
  `outcome` 回答"future 实际怎么结束的"。关停期优雅返回的记录是 `Cancelled + Returned`——
  既不假装它是被强杀的，也不把"关停期间的退出"混进正常的 `Completed`。
- **单化身**：`live` 账本对每个 key 至多一条；重启路径也是"先收旧、收干净才起新"，
  旧化身在收尾窗口内不肯结束就拒绝重启（返回错误，**不**起第二个化身）。
- **不留 detached**：所有 spawn 进同一个 `JoinSet`，运行期按 `Id → key` 记账；
  关停时收干（或 abort 后记清谁没回来）。
- **预算只放在一处**：`ShutdownBudget` 的每段都是绝对上限，L1 `total` 约束整段；
  二次信号只把剩余等待压到 `forced_phase`，**不刷新**任何截止时间。
- **诚实上报**：`stopped` 只收"停止之后在预算内结束"的任务；`still_running` 是 abort 之后仍没被
  观察到结束的（例如同步阻塞在 await 点之外）；`resource_failures` 记录关闭超时。
- **panic 依赖 `unwind`**：`JoinError::is_panic` 是 `Panicked` 分类的唯一来源（根 `Cargo.toml` 里
  显式写了 `panic = "unwind"`，不要改成 abort）。

## 测试形态

- 用 `#[tokio::test(start_paused = true)]` 的虚拟时钟：预算、退避、超时都是确定性的，不靠墙钟。
- 只有一个用例用真实时钟 + 双 worker：`shutdown_reports_still_running_task_honestly`——
  它要构造"同步阻塞、不在 await 点上"的任务，这正是 abort 收不回来的真实形态。
- 跨 runtime 的用例也不能用虚拟时钟（暂停的时钟同步不到另一个 runtime 的唤醒）。
- 断言只读 `ShutdownReport.records` 与 `broadcast` 通道，不装全局 subscriber。
