# `{{crate_prefix}}-worker`

周期任务面的**样板**。整个 crate 只有一个面、一个要填的函数。

它存在的理由不是"模板得有个后台任务"，而是：一个后台任务面有一组**要么全对要么全错**的形状
（返回 future 不自己 spawn、取消优先、热配置每 tick 现读、先回执再等提交），这些形状写对一次
比在文档里讲三遍有用。第二个周期任务照着这个抄，抄错的余地很小。

## 它刻意不做的事

- **不 spawn。** `TickerPlane::build` 返回一个 `PlaneFuture`，交给谁去跑是 `app` 的决定。
  清单里的 `tokio` 连 `rt` feature 都没开——能 spawn 就会有人 spawn，少给一个 feature 比写一条
  纪律可靠。
- **不建 runtime、不装 subscriber。** 前者同上；后者是：库层一旦能装 subscriber，就能在被
  别人 `use` 的时候擅自改全局日志。
- **不碰存储。** 邻接表明令禁止 `worker → storage`。后台任务要读写数据时，走 `app` 注入
  的 `Arc<dyn core::Storage>`——那是门面类型、住在 `core`，加一条 build 边等于把具体后端拖进任务面。
  这个模板里还没有那样的任务，所以**连注入口都没有预留**：第一个真需要它的人再加一个参数，
  比现在猜一个形状准。
- **不定义新的错误类型。** 面的失败面是 `core::task::PlaneError`。这一层再包一层只会让退出报告
  多一次转译。
- **不再钳一次配置。** `tick_interval` 由三段式管线保证不小于 `MIN_DURATION`，而启动与重载走的是
  **同一条**管线。再钳一次就等于"什么算合法间隔"在两个地方各有一份答案，而两份答案迟早
  会不一致——到那天，钳位记录说的是一个值、实际睡的是另一个值，排障的人会先怀疑日志。

## 公共出口

这份清单与源码里的公共项**两侧集合相等**，由 `make check` 的 `test` 门禁比对（实现在 `testkit/tests/discipline.rs`）。
本 crate 没有 feature，所以只有一份清单。下面那对 `exports:begin` / `exports:end` 注释是给门禁
读的：它只认这两行之间的内容。下面那张表里的 `SPEC` / `build` 是 `TickerPlane` 的**关联项**，
不是 crate 的出口，所以它落在标记之外——这正是需要显式标记而不是让扫描器猜的原因。

<!-- exports:begin -->

- `TickerPlane`

<!-- exports:end -->

`TickerPlane` 是个纯命名空间——没有字段，也不需要被构造出来。面的状态全在 `build()` 返回的那个
future 里，那正是它该待的地方：装配层拿不到它，也就无从在运行期改它。它身上只有两样东西：

| 名字 | 是什么 | 为什么在这一层 |
| --- | --- | --- |
| `SPEC` | 登记规格（`TaskName::Ticker` + `ShutdownClass::Abortable`） | `TaskSpec` 的文档说得很直接：「"这个面需不需要优雅收尾"只有写这个面的人知道」。让 `app` 在登记时替它猜一个，等于把那句话作废 |
| `build` | 造 future，不 spawn、不做任何可失败的事，所以不返回 `Result` | 见上一节第一条 |

这一层对外本来只需要 `build` 一个出口。`SPEC` 是后加的，理由如上。

## 一次 tick 的形状

```text
loop {
    let snapshot = config.load();                 // 现读，不缓存
    let interval = snapshot.config().worker.tick_interval;

    select! { biased;                             // 取消永远在第一臂
        cancel.cancelled() => return Ok(()),
        sleep(interval)    => {}
    }

    tick(seq, interval, &snapshot).await?;        // 业务写在这里
}
```

要填的只有 `tick`。其余每一行都有一条对应的用例盯着（见下）。

### 间隔什么时候生效

**先读后睡**。所以在任一时刻，已经睡下的那一轮早就把旧值取走了；重载不叫醒它，这一轮仍按旧间隔
走完，真正用上新值的是它之后的那一轮。延迟上界因此是**一个旧间隔**。

这条上界是故意换来的。要做到"改了立刻生效"就得持有一个可重建的 timer，而风险正落在那上面:
若在回执之后用新配置重建 timer，就会有一个可失败的步骤落在提交门**之后**——一个本该拦在启动期的
错误变成了运行期的半死状态。现读只有 `load()` 一个动作，没有可失败的重建步骤，那一整类问题在
结构上不存在。

### `enabled` 不在这一层读

`worker.enabled` 是**半热**的：它决定"这个面在不在"，不是"这个面这一轮做不做事"。判断因此发生在
装配期——`app` 决定要不要登记这个面，关掉时它根本不会被构造出来。

放在这一层读的话，会得到一个登记了、有 `TaskSpec`、会进关停报告、却什么都不做的空面——那正是
最难排查的一种"看起来在跑"。

### 回执立刻发，然后才等提交

这个面**没有什么要准备**：没有端口要 bind，没有连接要建。那为什么还要发回执？因为提交门的判据是
"登记过的面全部回执"这个谓词，谓词的定义域是**登记集**，不是"有准备工作的那些面"。让某些面免于
回执，就等于把"哪些面算数"变成一份要人维护的名单，而名单是会漏的。

等提交那一步必须 `select!` 取消，不能直接 `await`：启动**失败**路径（`abort_boot`）只做"取消 →
短宽限收割 → 关存储"，**不发** `Draining`，阶段停在 `Starting` 上。只等阶段的话这个面会一直挂着,
最后靠 abort 收掉——一次干净的启动失败在退出报告里会变成 `Cancelled`，看起来像一次超时。

### tick 失败会拉停整个进程

`tick` 返回 `Err` → 面返回 `Err` → first-failure 拉起全进程关停。这是**故意**选的响亮
默认值。

另一种写法（记一条日志然后继续下一轮）会让这个面的 `Result` 变成一句空话：一个永远不返回 `Err`
的面，和一个已经坏掉但还在转的面，在退出分类里长得一模一样——而那套四分类存在的全部意义
就是把这两者分开。**偶发**失败（网络抖动、锁竞争）属于 `tick` 内部该自己咽下去的事。

## 依赖

| 依赖 | 用到的 feature | 为什么 |
| --- | --- | --- |
| `{{crate_prefix}}-core` | — | 面的全部词汇。**唯一**一条成员边 |
| `tokio` | `time`, `macros` | `sleep` 与 `select!`。**没有** `rt` |
| `tokio-util` | — | `CancellationToken` |
| `tracing` | — | 见下 |

**这一层必须自己打日志，与 `storage` 那条"一条事件都不发"的纪律正好相反。** 不是双标：存储的失败
能通过 `StorageError` 上抛到 `app` 的唯一调用点去记，tick 没有这样的上抛点——不在这里记就在任何
地方都记不到。

级别选 `info` 而不是 `debug`：这是模板里唯一一个"改了配置立刻能观察到效果"的行为，
改一次 `tick_interval` 再发一次 SIGHUP，它就是观察热重载有没有真的生效最省事的入口；而一个
默认看不见的样板举不了任何证。使用者把业务填进来之后降成 `debug` 完全合理。

`ticker_tick` 这条事件里最有用的字段是 `generation`：它说的是"这一拍用的是第几版配置"，于是
"reload 报了成功，但面还在用旧值"这件事从日志上直接看得出来，不需要靠间隔去推断。

## 测试

全部在 `tests/`——这一层能断言的东西**全是外部可观察**的（回执有没有发、future 什么时候返回、
日志里有几条 `ticker_tick`、每条带的是第几版配置），没有什么值得从内部测。

```bash
cargo test -p <你的项目名>-worker
```

| 用例 | 钉的是 |
| --- | --- |
| `the_plane_acks_before_the_commit_and_does_nothing_until_running` | 回执先于提交；提交前不处理业务 |
| `a_boot_that_never_publishes_running_still_lets_the_plane_return_cleanly` | `abort_boot` 路径不挂死（阶段停在 `Starting`） |
| `shutdown_while_draining_is_also_a_clean_return` | 正常关停路径同样是 `Ok` |
| `it_ticks_once_per_interval_and_never_before_the_first_one_elapses` | 先睡后 tick，一个间隔一拍 |
| `a_reloaded_interval_takes_effect_after_the_round_that_already_read_the_old_one` | 延迟上界是一个旧间隔——逐拍核对 `generation` 与 `slept_ms` |
| `cancellation_wins_over_a_tick_that_is_due_at_the_same_instant` | `biased` 的落点 |
| `the_spec_says_abortable_and_the_plane_owns_that_answer` | `SPEC` 属于这一层 |

两件与别处不同的事：

- **用例跑在暂停的时钟上**（`start_paused = true`）。默认间隔是 30 秒起步的，靠真实等待一条用例
  就得跑半分钟，而且"等多久算够"会随机器负载漂。暂停之后时间只在 `tokio::time::advance` 里前进,
  断言因此是确定性的，不是概率性的。代价写在清单里：dev-dependency 的 `tokio` 要 `rt` 而**不是**
  `rt-multi-thread`——`tokio::time::pause()` 只在 current-thread 调度器里成立。
- **夹具里不 `panic!`。** `clippy.toml` 的 `allow-panic-in-tests` 是结构性豁免，只认 `#[cfg(test)]`
  模块和 `#[test]` 函数体；集成测试的辅助函数两样都不占，`-D warnings` 下一句 `panic!` 就是一次
  门禁失败。所以 `republish_interval` 把判定用返回值交回 `#[test]`——那里本来也更适合放断言，
  失败位置指向用例而不是指向夹具。

`lib.rs` 末尾那行 `#[cfg(test)] use service_testkit as _;` 和 `tests/ticker.rs` 里的
`use tracing as _;` 都是给 `unused_crate_dependencies` 交代的：dev-dependency 会被链进 **lib 的
test 目标**，普通依赖会被链进**集成测试目标**，任一处没用到就是一条警告。`as _` 只满足链接检查,
不引入任何可命名的东西。另一条路是编几个用不上的单元用例把夹具用起来——那等于为了一条 lint 往库
里塞假断言。

本 crate **一条 doctest 都没有**，全模板都没有：doctest 里的 `use` 必须写 crate 真名，而真名由
模板变量展开，写死任何一个都会在别人展开之后失效。示例代码一律标 `text` 围栏——标 `ignore` 仍然
会被计成一条 doctest。`cargo test --doc` 的计数必须是 0/0/0。

## 照着加第二个周期任务

1. 在 `core` 的 `TaskName` 里加一个变体（面名是**枚举**，不是字符串——拼错在编译期就红）。
2. 复制 `ticker.rs`，改 `SPEC` 的名字与 `ShutdownClass`，把 `tick` 换成你的活儿。
3. 在 `app` 的装配里 `spawn_on` 它，配一个 `ack_channel`。
4. 如果它要读写数据，给 `build` 加一个 `Arc<dyn Storage>` 参数——**不要**给这个 crate 加
   `{{crate_prefix}}-storage` 依赖。
