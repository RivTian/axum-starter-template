# {{crate_prefix}}-app

装配层（composition root）。产出二进制 `{{crate_name}}`：解析 CLI、装 tracing、加载配置、
建 runtime、装配共享状态、注册顶层任务、守信号、统一关停。

## 边界

- 依赖方向：`app -> {api, worker, storage} -> core`。它是**唯一**允许同时看见各面的 crate。
- **唯一装 tracing subscriber 的地方**（`boot::init_tracing`）；其余 crate 只 `use tracing`。
- **唯一决定任务 spawn 到哪个 runtime 的地方**：面只返回 future，绑定由这里解析。
- **不写业务逻辑**：这里只有装配顺序与生命周期编排。加一个面时改的是
  `register_runtime_tasks` 的一段注册，不是往这里塞循环体。
- 进程内**没有任何业务全局态**：全部经 `RuntimeState` 注入。这是测试并行化的前提。

## 目录

```text
src/
  main.rs     进程入口：CLI → tracing → 配置 → runtime → block_on(boot) → 关 runtime → 退出码
  cli.rs      --version / --config；其余参数忽略
  rt.rs       Runtimes / Executors：按 [runtime] 建主与附加 runtime，解析面的绑定
  state.rs    RuntimeState：装配一次、注入各面
  boot.rs     boot_strap：存储 → 共享态 → register_runtime_tasks → monitor；init_tracing
  signals.rs  monitor：信号循环（SIGINT/SIGTERM/SIGHUP）+ first-failure + 统一关停序列
```

## 关键决策

**配置先于 runtime。** 与「一个 `#[tokio::main]`」的差别只有这一处：runtime 的形状由配置
决定，所以配置加载前移到 `block_on` 之外。加载本身是同步的，前移零代价。

**附加 runtime 在 `block_on` 之外同步关。** 在 async 上下文里 drop 一个 `Runtime` 会 panic；
`shutdown_background` 不 panic 但不等任务收尾，顺序就看不出来了。`main` 在 `block_on`
返回之后逐个 `shutdown_timeout`：附加 runtime 逆序先关，主 runtime 最后——共享资源
（存储池）活在主 runtime 上。

**附加 runtime 只有多线程一种形态。** 模板里没有任何线程会对附加 runtime 调 `block_on`，
而 `current_thread` 的 runtime 要有人 `block_on` 才轮询，光 `Handle::spawn` 进去的任务
永不执行、`await` 它就是挂死。要单线程隔离写 `worker_threads = 1`。
`rt.rs` 模块头列了三条跨 runtime 规则，加面前先读那一段。

**未声明的 runtime 名字是错误，不回落。** 回落会把「配置写错」藏成「性能不达预期」。
配置加载期已拦一次，`Executors::resolve` 是绕过管线直接装配时的最后一道网。

**`http` 永远最后注册。** HTTP 开始接流量时其余面必须已就绪，否则请求打到半装配的面。
其他面一律插在它之前。面的 `enabled = false` 时不注册并 `warn!`——「接口全都在但数据
不更新」是最难猜的一种故障，关掉必须在日志里喊出来。

**启动失败就中止，不半装配上线。** 存储初始化在任务注册之前（带着不可用的存储跑起来，
故障会推迟到首次写入才暴露）；注册失败走 `abort_boot`：取消已注册者 → 短宽限收割 →
关库 → 返回**原始**错误。此处不发 `ShuttingDown`——订阅者尚未就绪，广播只会石沉大海。

**所有退出路径共用一条关停序列。** 广播 `ShuttingDown` → 根令牌取消 → 10s 限时收割
（超时 abort）→ 关库。存储最后关：宽限期内的任务可能还在做最后一批写入，提前关池会把
正常落盘变成一堆连接错误。面内部若有二级收割，预算要**短于** 10s 并留余量，内外两层
取同值会让内层吃满、外层没时间收尾。

**没有注册任何顶层任务时留痕后正常退出。** 否则 `next_exit()` 立即返回 `Empty`，
被 first-failure 判成异常。

**日志只有 compact 单行一种形态。** 不留格式开关：`key=value` 采集器切得动，人也扫得动。
`RUST_LOG` 以 crate 边界过滤
（`RUST_LOG={{crate_prefix_snake}}_worker=debug`）；线程名进日志，多 runtime 时一眼看出
一条日志来自哪个 runtime（`compute-0`）。

**两个参数不值一个 clap。** `--version` 与 `--config`，其余一律忽略——部署脚本带着的
历史参数不该让进程起不来。配置路径取值顺序：`--config` > `${{env_prefix}}_CONFIG` >
缺省布局 `<安装根>/config/{{crate_name}}.toml`。

**安装根锚在可执行文件上，不是进程 cwd，也不跟着 `--config` 走。** cwd 不是进程自己的
属性：systemd 拉起的服务 cwd 通常是 `/`，按 cwd 推缺省是去建 `/config/`；从另一个目录启动
同一个二进制，又会在那里另建一套目录，而「换个目录启动就读到另一份配置」查起来没有线索。
锚在二进制上则跟着二进制走——`cargo run` 落 `target/debug/`（`cargo clean` 会一起删，本地想
留就把路径指到仓库里），装出来的服务落安装目录。`--config` 与环境变量只挪配置文件，不挪
安装根：单元文件里把缺省路径显式写出来是常见做法，不该因此让 `data/` 换个地方落——
「显式写出缺省值反而改变行为」是最难查的一类故障。两者里的相对路径仍相对 cwd：那是人在
shell 里敲的，就该是 shell 的语义。`locate_config` 一次返回 (安装根, 配置路径)，两者都要问
`current_exe()`，失败也在同一处报。

## 测试形态

同文件 `#[cfg(test)]` 单测，分三类：

| 类别               | 形态                                            | 钉住什么                                                                             |
| ------------------ | ----------------------------------------------- | ------------------------------------------------------------------------------------ |
| `cli`              | 纯同步                                          | 取值优先级、未知参数被忽略                                                           |
| `rt`               | **同步** `#[test]` + 真 `Runtimes`              | 绑定生效（探针报告自己在哪条线程上）、单 worker 也跑得起来、未声明名字报错、关停顺序 |
| `boot` / `signals` | `#[tokio::test]` + `temp_storage` + `test_port` | 关闸时少一个任务且日志有 warn、空监管器留痕退出、关停序列的日志顺序                  |

`rt` 的测试**必须**是同步的：`Runtime` 不能在 async 上下文里建与关。它里面那条
`a_single_worker_runtime_still_runs_its_tasks` 是防回归的——`current_thread` 形态一旦
被重新引入，同样的探针会永远挂在 `await` 上。

断言日志顺序用 testkit 的 `LogCapture`（按线程收集，并行用例互不串味）；库用
`temp_storage`（一用例一个临时 SQLite 文件库）；端口用 `test_port`（按 pid 派生 + 用例内
偏移），经测试专属配置文件的 `[http].port` 喂给正常装配——**不为测试改生产 bind 逻辑**。

跨线程 async 任务打的日志收不进 `LogCapture`。要断言那类日志，断它的同步注册段，
或让任务把结论发到 `EventBus`。

```sh
cargo test -p {{crate_prefix}}-app --all-targets
```
