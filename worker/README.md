# {{crate_prefix}}-worker

示例任务面 `ticker`。它是「一个面级 crate 该长什么样」的参照：按周期打点、记指标、
打日志，三件事刚好够演示一个顶层任务面的全部纪律。

把它改成真实的面，或者删掉它——但删之前先把下面这几条搬到你的面里。

## 边界

- 依赖方向：`worker -> core`。**不依赖 api**，两个面的衔接只经 core 里的共享态与事件。
  需要存储就加 `storage` 边并在评审说明；需要管一组长活单元就加 `reconcile` 边。
- **窄门面**：对 app 只有一个入口 `ticker`（即 `ticker::run`），内部结构一律不转出。
- **不 `tokio::spawn` 自己**：入口是 `async fn`，返回的 future 是 `Send + 'static`
  （入参全是句柄与 `Arc`）。spawn 到哪个 runtime 是装配层的事。
- **不初始化 tracing、不落库细节**。

## 目录

```text
src/
  lib.rs      窄门面 + 面级 crate 的五条纪律
  ticker.rs   示例主循环 + 三条节奏测试
```

## 关键决策

**取消是 `select!` 的第一分支，且 `biased`。** 关停优先于新工作：等待期收到取消立即退出，
不等当前周期走完。没有 `biased` 时 tokio 随机挑分支，关停会变成概率事件。

**可热字段每 tick 现读。** `config.current().ticker.interval_ms` 在循环体内取，不在循环外
取一次——后者会让热重载对已经跑起来的面完全无效。新值从**下一次等待**开始生效，
在途的那一拍仍按旧周期到期，这是 `sleep` 的语义，测试里钉死了。

**旁路纪律：业务失败不让循环退出。** 动作失败只记 `metrics.<面>.last_error` 与日志，
不向上传播。循环退出 = first-failure = 进程退出，那是留给「面本身坏了」的信号，
不是留给一次业务失败的。

**指标只加真实会写的字段。** 加一个面就在 `core::metrics` 加一个 `XxxStats` 与对应的
`XxxSnapshot`，只增不改名——改名等于丢失与历史指标的对照关系。

**启动闸是半热的。** `[ticker].enabled` 只在启动时读一次：关着启动不会被热重载补拉起，
重开要重启进程。装配层在关闸时会打一条 `warn!`。

## 测试形态

三条 `#[tokio::test(start_paused = true)]`：模拟时钟，不等真实时间，一条 350ms 的节奏
断言在真实世界里几微秒跑完。

| 测试                                                   | 钉住什么                                     |
| ------------------------------------------------------ | -------------------------------------------- |
| `ticks_at_the_configured_interval`                     | 过 350ms 恰好 3 拍（100 / 200 / 300）        |
| `cancellation_exits_without_waiting_for_the_next_tick` | 周期给 60s，取消后 10ms 内必须退出           |
| `hot_reloaded_interval_applies_from_the_next_wait`     | 在途一拍按旧周期到期，新周期从下一次等待生效 |

两个装置值得照抄：

- **推进时钟用 `sleep` 而不是 `time::advance`。** 暂停的时钟下运行时空闲时会自动推进到
  下一个最早的定时器，被测任务的每一拍都按顺序到期并重新武装；`advance` 只触发推进前
  已存在的定时器，中途新武装的那些不会被补上，断言会少几拍。
- **取消用超时来断言，而不是等它退出。** `timeout(10ms, task)` 远早于 60s 的那一拍：
  取消若没抢在 `sleep` 之前，这条测试就红，而不是慢。

配置落临时目录、用完自己删；不引 testkit（面级 crate 的 dev-dependencies 保持最小，
这里只多一个 tokio 的 `test-util` feature）。

```sh
cargo test -p {{crate_prefix}}-worker --all-targets
```
