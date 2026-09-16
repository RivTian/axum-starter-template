# `{{crate_prefix}}-app`

装配层。全仓库唯一有 `main` 的 crate，也是唯一一处同时看得见 `core` / `storage` / `worker` /
`api` 四层的地方（分层邻接表里，`app` 有四条出边，反向一条都没有）。

它的职责可以压成一句话：**把进程环境变成一个跑起来的服务，再把它干净地停下来，最后如实上报
这一次到底算不算干净。** 三件事各自都不难，难的是它们之间的**顺序**——而顺序写错的症状几乎
都是"看起来正常"：日志干净、进程起来了、写的却是另一个目录下的库；Ctrl-C 之后 `runtime
stopped` 照常打印，但有任务是被掐掉的，退出码仍然是 0。

所以这一层的每一条纪律都是冲着"顺序错了但看不出来"写的。

## 只有一个入口

```text
pub fn run<F, S>(env: io::Result<ProcessEnv>, stops: F) -> RunReport
where
    F: FnOnce() -> io::Result<S>,
    S: Stream<Item = ProcessSignal>,
```

它**不读 `std::env`、不装 `ctrl_c`、不调 `exit`、不返回 `Result`、不 panic**。进程事实从参数
进来，一次运行的所有结局——包括启动失败——都是 `RunReport` 的一种形态，退出码由 `exit_code`
从它算出来。

代价是 `main.rs` 要多写一行。收益是整条启动—运行—关停路径能在一个用例里跑完，既不需要真起
进程、也不需要真发信号——[`tests/run.rs`](tests/run.rs) 就是这么跑的：塞一条写死的信号流，
断言 `RunReport` 与退出码。

`main.rs` 因此是**一条没有分支的表达式**，而且门禁会扫它：里面不得出现 `if` / `match` / 循环。
每多一个分支，就多一条只有真起进程才能覆盖到的路径。

### `stops` 为什么是函数而不是一条流

实测：`tokio::signal::unix::signal()` 在 runtime 之外调用会 **panic**
（`there is no reactor running`）。而 runtime 是 `run` 自己建的，`main` 手上没有。

写成 `run(env, stops: impl Stream)` 是更自然的形状，但那个签名跑不起来。注入点并没有变，
变的只是注入的是**值**还是**产生值的函数**。

## 启动的固定顺序

```text
① 进程事实         ProcessEnv            ← 由 main 抄下来传进来
② 命令行           cli::parse            ← --help / --version 在这里就走
③ 安装根           paths::install_root_from
④ 配置             bootstrap::load       ← 三段优先级
⑤ subscriber       telemetry::init       ← **配置之后**
⑥ runtime          RuntimeSet::build
── 进入 block_on ──
⑦ 信号             stops()               ← 必须在 runtime 里，且**早于**装配
⑧ 装配 + 起任务    assemble → launch
⑨ 编排             r#loop::orchestrate
── block_on 返回 ──
⑩ 关 runtime       RuntimeSet::shutdown  ← 必须在 block_on 返回**之后**
```

四个位置是钉死的，动一个就会有东西悄悄坏掉：

| 位置 | 钉死的理由 |
| --- | --- |
| ⑤ 在 ④ 之后 | 过滤器、格式、着色全写在配置文件里。反过来做（先装默认的，读完配置再换）要么需要一个可重装的全局（本模板不留全局可变状态），要么需要 `reload::Handle`——那会让"日志格式"成为唯一一个能在运行期被换掉却不走配置热重载的东西 |
| ⑦ 在 ⑧ 之前 | 注册之后、第一次 poll 之前到达的信号**不会丢**（实测）。而装配可能很慢（建连接池、绑端口），那段窗口里收到的 SIGTERM 若走默认处置会直接杀掉进程、跳过全部收尾 |
| ③ 只算一次 | 安装根是配置位置与数据位置的**共同**锚点。算两次就有机会算出两个答案 |
| ⑩ 在 `block_on` 之后 | 在一个 runtime 的上下文里关这个 runtime 会 panic，而且 panic 发生在关停路径上——现场往往已经没人看了 |

⑤ 的代价是 ①～④ 的失败没有结构化日志。这里的处理是 `fail_before_telemetry`：用**默认**遥测
配置临时装一个 subscriber，专门把这条失败发出去，然后立刻返回。于是「没有结构化日志」缩小成
「这条日志用的是默认格式」，而不是「这条日志不存在」。

①～⑤ 里攒下来的事实（回落了哪个过滤器、配置从哪来、写没写模板、钳了哪些值）由 `replay` 在
subscriber 就绪之后**补记一次**。产生方一律"把事实作为数据交出来"而不是就地记日志——
同一件事记两遍会让运维以为发生了两次。

## 关停的固定序列

```text
publish(Draining)        ← 公告先于任何取消
info!(shutdown_started)
root.cancel()            ← 级联到所有 child token
harvest(plan.harvest)    ← 只等 ShutdownClass::Graceful 的面自己收尾
abort_all(); reap(reap)  ← 独立的第二段预算；全部任务一律进这一段
storage.close(close)     ← 存储最后关
── block_on 返回 ──
runtimes.shutdown(rt)    ← 同步上下文里执行
```

四段预算来自同一份 `Plan`，而 `Plan` 是**一个绝对 deadline 沿链传递**算出来的，不是四个独立的
超时。于是"每段都没超时、加起来超了"这件事不成立。

**启动失败走的是同一条序列**（`Sequence::drain` 只有一份），差别只有两处：发的相位是
`Phase::Aborting` 还是 `Phase::Draining`，以及最后产出哪一种 `RunReport`。不给启动失败
另写一条短路径，是因为两条序列迟早会分叉——而分叉那天被漏掉的一定是"关存储"。

第二次停止请求**收紧**总边界（`min(原计划, now + escalate)`），不重新计时、不换首因；第三次
立即强退。三次的语义全在 `lifecycle::stop` 那个纯状态机里，时钟是参数——所以「连按三次 Ctrl-C」
能在一个毫秒级的同步用例里验完。

## 它刻意不做的事

| 不做 | 为什么 |
| --- | --- |
| 不在 `main` 里分支 | 见上。门禁扫这个文件 |
| 不在任何地方 `std::process::exit` | `exit` 绕过 `Drop`，而关停的最后一段发生在 `block_on` 返回**之后**——在那之前 `exit` 掉等于把前三段预算全做成白工，日志上还看不出来 |
| 不把 `succeeded()` 做成一个布尔字段 | 六项合取，每一项都对应一类能单独发生的故障。压成一个 `bool` 并在某处 `report.ok = false`，等于在写入那一刻就丢掉"是哪一项不成立" |
| 不装 `tracing-subscriber` 的 `reload::Layer` | 每条日志多过一层 `RwLock`，换来一个本模板不需要的能力——`[telemetry]` 整段都是冷字段，改了本来就要重启 |
| 不引参数解析库 | 参数表一共三项。不为观感引依赖 |
| 不在重载路径上用 `tokio::fs` | 它**就是** `spawn_blocking`，换来的不是"不阻塞"而是"阻塞在别人的线程上，外加一次跨线程调度"。约束这次读的是大小上限，不是异步 |
| 不在运行期重算"读哪个文件"/"环境变量是什么" | 启动时定死。重算要么得到同一个答案，要么意味着有人在运行期改了进程环境——而 `set_var` 在 Rust 2024 里是 `unsafe`，本模板一次都没用 |
| 不提供 HTTP 重载端点 | 换配置是运维动作，做成端点就得回答鉴权问题，而系统端点不鉴权。SIGHUP 的权限边界由操作系统给 |

## 模块

按"离进程有多近"排：

| 模块 | 内容 | 一句话纪律 |
| --- | --- | --- |
| `boot::env` | 读 `current_exe` / `args_os` / `vars_os` / `is_terminal` | 全仓库唯一读进程环境的地方。**分支数应当一直是 0**，门禁会数 |
| `cli` | 三个开关，手写解析 | 认得的开关缺值一律 fail-fast，不静默兜底 |
| `config::bootstrap` | 选文件、读文件、解析 | 全仓库唯一往磁盘**写**配置的地方，且只在启动路径、只在缺省位置那一档 |
| `config::reload` | SIGHUP 之后的事务式整批换 | 这个文件里没有 `std::fs`；single-flight 由 `&mut ConfigPublisher` 给，不是锁给的 |
| `telemetry` | 装 subscriber | 产品代码里唯一（`testkit` 里还有一处，但它 dev-only、进不了二进制）。过滤器写错退回默认并**留一条 `warn`** |
| `rt` | 建 runtime | 全仓库唯一。`RuntimeSet` 是所有权、`Executors` 是只读句柄 |
| `signals` | `tokio::signal` → `ProcessSignal` | 全仓库唯一。只转发，一个判断都不做 |
| `boot::assembly` | 纯构造 → 唯一 spawn 点 | 两段式：手上是 `Assembled` 就说明还没有任务在跑 |
| `boot::gate` | 提交门 | prepared ≠ committed。提交条件是一个能被直接断言的谓词，不是分支顺序 |
| `lifecycle::stop` | 纯状态机 | 时钟是参数 |
| `lifecycle::report` | 纯数据 + 一个合取判据 | 退出码只由它算 |
| `lifecycle::r#loop` | 唯一有副作用的那个 | 把上面两个的结论落到 supervisor、存储和 runtime 上 |

## 公共出口

这份清单与源码里的公共项**两侧集合相等**，由 `make check` 的 `test` 门禁比对（实现在 `testkit/tests/discipline.rs`）。
本 crate 没有 feature，所以只有一份清单。全部在 crate 根，没有公开模块。
下面那对 `exports:begin` / `exports:end` 注释是给门禁读的，一行一个名字。

<!-- exports:begin -->

- `ProcessEnv`
- `ProcessSignal`
- `RunReport`
- `StartupFailure`
- `StopCause`
- `exit_code`
- `os_signal_stream`
- `run`

<!-- exports:end -->

八项里只有 `run` 是这一层做的事。其余七项分成两类：三个是**喂给它的**（`ProcessEnv`、
`ProcessSignal`、`os_signal_stream`），四个是**读它结果用的**（`RunReport`、`StartupFailure`、
`StopCause`、`exit_code`）。

`RuntimeSet`、`Executors`、`Assembled`、`Running` 一个都不出 crate：它们是 `run` 内部的编排
状态，露出去就等于允许别人在 `run` 之外再拼一条启动序列——而这一层全部的价值就在于那条序列
只有一份。

## 几个不显眼但重要的形状

- **`run` 连 `ProcessEnv::capture` 的失败一起收下**（参数类型是 `io::Result<ProcessEnv>`）。
  `current_exe()` 拿不到时安装根就无从谈起，而那条失败和其他启动失败没有本质区别，不该逼着
  `main` 去分支处理。
- **`bin_name` 是 `&'static str`，由 `main.rs` 的 `env!("CARGO_BIN_NAME")` 传进来。**
  它是 `<PREFIX>_CONFIG` 里那个 `<PREFIX>` 的唯一来源，也是 `/v1/info` 那个 `service` 的唯一
  来源——于是整个 Rust 源里不需要出现任何项目名字面量。收 `&'static str` 而不是 `String`，是为了
  堵死"拿 `argv[0]` 填它"：那样一来，把可执行文件改个名就会换掉环境变量前缀和服务身份，
  一次改名变成一次静默的配置迁移。
- **`RuntimeSet` 是 `!Send` 的**（靠一个 `PhantomData<*const ()>`）。编译器因此拦住了那类最阴的
  缺陷：把 `Runtime` 带进 async 上下文里 drop。
- **`assemble` 里存储是最后一件被打开的东西，之后再没有可失败的步骤。** 于是不存在"存储已经
  开着、但我要返回 `Err`"这条路径——端口被占时不该有人先跑一遍数据库迁移再回滚。这条不变量
  **靠排序**维持，不靠类型；往后面加可失败步骤的人必须同时回答"存储谁来关"。
- **提交门不用 `_ = ready(()) => break` 那种兜底分支**。那等于把提交条件
  寄存在分支顺序里：有人把那一臂往上挪一行，提交就会发生在回执之前，而这不会有任何编译错误
  或测试失败，症状是偶发的"服务刚上线就 502"。
- **`StartFailure` 不外露。** 它只是为了让 ⑦ 与 ⑧ 的失败能共用一个 `?`——它们之后的处理完全
  一样，写两层 `match` 的话两份「启动失败」处理迟早会分叉。

## 依赖

完整理由逐条写在 [`Cargo.toml`](Cargo.toml) 的注释里。这里只列三件在别处看不到的事：

- **四条出边**（`core` / `storage` / `worker` / `api`）只有这一层有。
- **`tokio` 的 `rt-multi-thread` 与 `signal` 只有这一层能写进 `[dependencies]`。** 于是"谁能建
  runtime""谁能订阅信号"是清单层面的事实，不是一条要人遵守的纪律。别的层在
  `[dev-dependencies]` 里开 `rt-multi-thread` 是另一回事——用例要真的跑起来就得有个调度器，
  而 dev-dependency 进不了任何二进制。
- **这里没有 `serde`。** 初稿写过，理由是"`deserialize` 这个动作发生在本 crate"——但
  `serde_path_to_error::deserialize` 是自由函数，`app` 里没有任何一处需要把 `serde` 的名字写
  出来。`unused_crate_dependencies` 当场指出了这点，依赖被删掉了。

`src/main.rs` 与 `tests/` 顶上各有一条 `#![allow(unused_crate_dependencies)]`：这条 lint 是
**逐 target** 判定的，而那两个 target 只用到自家 lib。不写十几行 `use tokio as _;` 去哄它——
那些语句不表达任何东西，正是那种「为观感加的构造」。

## 测试

```bash
cargo test -p <你的项目名>-app
```

### 一个测试二进制里只能有一次 `run`

`run` 会调 `tracing::subscriber::set_global_default`，而那个槽位**每个进程只有一个**。第二次
调用拿到 `AlreadyInstalled`，`run` 会把它变成一次 telemetry 启动失败——于是第二条用例验的不再
是它想验的东西，**而且它会不会红取决于测试的执行顺序**。

同一条约束还有另一半：`service_testkit::LogCapture` 装的也是那个槽位。所以「调 `run`」与
「断言日志」在一个二进制里只能有一个。本模板的分法是：

| 要验的东西 | 放哪 |
| --- | --- |
| 日志顺序、日志内容 | `app` 的**单元测试**（不经过 `run`，`LogCapture` 能正常工作） |
| 整条 `run` 的结论 | `app/tests/` 下**一个文件一条**，断言 `RunReport`、退出码与文件系统效果 |

### 在哪能找到什么

| 文件 | 钉的是 |
| --- | --- |
| `boot/env.rs` | 进程事实抄得对；非 UTF-8 的环境变量读作"不存在"而不是被改写 |
| `cli.rs` | 缺值 fail-fast；下一个开关不会被当成值吞掉；usage 里不出现项目名字面量 |
| `config/bootstrap.rs` | 三段优先级的顺序；缺省位置首启动会创建、显式路径永远不创建；错误带字段路径；相对路径锚在安装根；超大文件失败而非截断 |
| `config/reload.rs` | 删掉文件再 SIGHUP **不会**静默重建；冷字段变了整批退回；`Debug` 不泄漏环境变量取值 |
| `telemetry.rs` | 两种格式各自的字段布局；着色只由注入的终端标志决定；过滤器非法时回落**并且**说出来 |
| `rt.rs` | 缺省配置只建一个 runtime、不多占一组线程；`resolve` 是查表不是工厂；附加 runtime 逆序关、主 runtime 最后 |
| `signals.rs` | 重载不是一次停止请求；每个变体有各自的名字 |
| `boot/assembly.rs` | `assemble` 之后 drop 不留任务；`bind` 失败不先开存储；`launch` 是唯一 spawn 点 |
| `boot/gate.rs` | 提交永不先于未到的回执；面死在回执之前要带着原因中止；回执端被 drop 时不干等 |
| `lifecycle/stop.rs` | 第二次只收紧不延长（哪怕来得很晚）、保留首因；第三次强退且保持强退 |
| `lifecycle/report.rs` | 六项合取每一项都能单独让它失败；强杀不会被报成一次干净关停 |
| `lifecycle/loop.rs` | 公告先于取消、存储最后关；SIGHUP 在排空期被忽略；信号流断了只报一次 |
| `tests/run.rs` | 一次完整起停的**结论**：报告说成功、退出码跟着走、每个在册的面都有一条 `Returned` 记录 |
| `tests/bad_config.rs` | 读不懂的配置必须起不来、说清是哪个键、且**不留副作用**（库文件没被创建）；顺带是"键名里的控制字符会被剥掉"的唯一端到端证据——不剥的话，一个带 `\n` 的键名能在日志流里伪造出一整条记录 |

关停预算的用例把时钟拨快（`tokio` 的 `test-util`），否则一条「30 秒宽限期」的断言就要真跑
30 秒，而且"等多久算够"会随机器负载漂。

本 crate **一条 doctest 都没有**，全模板都没有：doctest 里的 `use` 必须写 crate 真名，而真名由
模板变量展开，写死任何一个都会在别人展开之后失效。示例代码一律标 `text` 围栏——标 `ignore`
仍然会被计成一条 doctest。

## 想改这一层的时候

- **加一个新的面**：在 `core` 的 `TaskName` 里加变体 → 在 `assembly::assemble` 里造它（纯构造，
  不 spawn）→ 在 `Assembled::launch` 里 `spawn_on` 并配一个 `ack_channel`。提交门不用动：它数的
  是登记集。
- **支持非 unix**：`signals.rs` 在非 unix 上是 `compile_error!`，不是一个没验证过的实现
  （不声称没验证过的平台）。补齐只需换掉那个产生器——`run` 收的是 `impl Stream`，编排层
  一行都不用动。
- **改启动顺序**：先读上面那张表。四个位置各自绑着一个具体失效，不是排版选择。
- **加一个可失败的启动步骤**：看它落在 `assemble` 的第 ③ 步（打开存储）之前还是之后。之后的话，
  先回答"存储谁来关"。
