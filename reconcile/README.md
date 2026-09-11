# {{crate_prefix}}-reconcile

期望集 reconcile 框架。**可选**：它是 workspace 成员（永远参与构建与测试），但
默认没有任何 crate 依赖它。

它解决一个通用形状：一组由配置或库表决定的长活单元，配置变了就关旧建新、死了就
退避重建、反复死就放弃、超限就推迟。每租户 worker、每设备会话、每队列消费者都是
这个形状。示例面 `ticker` 是单个常驻循环，用不上这一层，所以默认不接。

## 边界

- 依赖方向单向：`reconcile -> core`。只用 core 的两样东西：时间基元
  (`util::{TimestampMs, now_ms}`) 与事件总线 (`events::{AppEvent, EventBus, EventStream}`)。
- 不含任何 SQL、不含任何面的业务语义。单元的依赖由**工厂**持有，框架对它们一无所知。
- 不 spawn 自己的顶层任务：`drive` 是一个 future，由面的顶层任务体 `await`，因而在
  `TaskSupervisor` 里仍然只占一个名额。
- 与 `core::task::TaskSupervisor` 是两层：supervisor 管顶层任务（每面一个，进程级
  first-failure），本 crate 管面内部的一组单元。

## 目录

```text
src/
  lib.rs      窄门面导出 + 「怎么接一个面」五步
  unit.rs     契约：UnitSpec / UnitFactory / UnitCtx / UnitExit / UnitStatus
  plan.rs     决策：plan_reconcile 纯函数 + RestartPolicy 退避曲线
  manager.rs  执行与记账：UnitManager（spawn / 取消 / 收割 / 死亡计数 / 快照）
  driver.rs   驱动循环：drive（事件唤醒 + 定时兜底）+ UnitSource + wake_on_plane
```

读的顺序就是这个顺序：契约 → 决策 → 执行 → 驱动。

## 关键决策

**决策与执行分家。** 对账的全部判断收在 `plan_reconcile` 纯函数里，管理器只执行清单。
纯函数拿不到 `JoinHandle`，不可能顺手 spawn/cancel，于是这些判断能用同步的输入输出表
测完——不起运行时、不等真实时钟。「等一等再断言」正是任务类测试不稳定的主要来源。

**六种情形的判定顺序是不变量。** 停 → 规格变更 → 还活着 → 已放弃 → 退避中 → 按死亡
重建。「规格变更」排在放弃之前是有意的：改配置是解除放弃的**唯一**手段，顺序一调就没了。

**收割超时即强杀。** 未收割的句柄一旦丢下，紧接着又为同一 key 拉起新化身，同一单元
就有两个实例在跑。宁可粗暴收场，不留双化身。口径与 core 的关停收敛一致。

**上限只扣活着的单元，余额先给重建。** 把僵尸算进上限会让故障时的名额反而收紧，恰好
卡住恢复。被挡下的进 `deferred` 报告，不烧死亡计数。

**唤醒是谓词，不是事件变体。** `drive` 收一个 `wake: impl FnMut(&AppEvent) -> bool`。
常规传 `wake_on_plane(<面名>)`，只认本面的 `DesiredSetChanged`。**谓词判假的事件既
不唤醒也不重置兜底期限**——deadline 在进入等待相时算一次，此后不重算。这一行堵死的是
自激热循环：面每次写库都发事件，若无关事件能落回外层循环，就成了「每轮写入 → 一次
全量巡检」。

**进度清零用每化身一个新计数器。** 「读数大于 0」精确等于「本化身干成过事」。用
「spawn 成功」代替会让每次都在第一轮前 panic 的单元永远死亡计数为 0，退避与放弃双双失效。

**`gave_up` 是边沿，只报一次。** 靠轮询快照对比拿不回这个时点，需要「放弃直到配置变更」
语义的面必须消费 `ReconcileReport`。

## 测试形态

| 层        | 形态                                           | 为什么                                           |
| --------- | ---------------------------------------------- | ------------------------------------------------ |
| `plan`    | 同步纯函数，手写时间戳                         | 不起运行时，六种情形与上限记账逐条钉死           |
| `manager` | `#[tokio::test]` 真实时钟 + 行为脚本枚举       | 收割、abort、死亡计数要真的 spawn 才能验         |
| `driver`  | `#[tokio::test(start_paused = true)]` 模拟时钟 | 兜底 tick 与事件唤醒的时序断言要「时钟一格未走」 |

两个装置值得照抄：`manager` 用 `Arc<()>` 探针的 `strong_count` 指认「卡死的 future 是否
真被销毁」；`driver` 用 `spin_until` 纯让出调度权地等计数，不碰时钟，够不到就 panic。

`driver` 的测试把退避基数取零：单元的 `next_restart_at_ms` 用墙钟，而测试跑在模拟时钟
上，几十个虚拟 tick 在真实世界只过去几百微秒——任何非零的真实退避窗口都会把重建挡在
所有虚拟轮次之外。退避的时序语义由 `plan` 层负责。

## 接进来要加一条依赖边

在用它的那个面级 crate 的 `[dependencies]` 里加
`{{crate_prefix}}-reconcile = { workspace = true }`，并在评审说明这条新边。接法见
`src/lib.rs` 模块文档的五步。
