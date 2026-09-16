# `testkit` — 测试夹具

**dev-only**：没有任何 crate 在 `[dependencies]` 里写它，只出现在 `[dev-dependencies]` 里。它进不了任何二进制。

它存在的唯一理由是「同一段绕坑代码不要抄五份」。里面的两类东西——日志捕获与临时安装根——都不是"通用测试工具"，而是本模板几条纪律的**取证手段**：那些纪律（停机公告先于取消、存储最后关、配置随安装根走）都是关于**顺序**和**位置**的，而顺序和位置只能靠观察进程的实际行为来证。

| 要证明什么 | 靠这里的什么 |
| --- | --- |
| 停机公告先于任何取消动作 | `LogCapture::position` 比较事件下标 |
| 存储在所有任务退出之后才关 | `LogCapture::last_position` |
| 配置解析随安装根走，不随 cwd 走 | `TempInstallRoot`，三份不同的锚点 |
| 坏文件 / 缺文件 / 坏权限下保留 last-good | `TempInstallRoot::write_config` |
| `open` 会把不存在的库建出来 | `TempDb`，路径给了但文件不建 |

## 它刻意不做的事

| 不做 | 为什么 |
| --- | --- |
| 不依赖除 `core` 以外的任何成员 | 分层邻接表里 `testkit` 那一行只有一个 ✔。夹具依赖被测层会让"测试基础设施"变成第七层，然后"改一层要动夹具、改夹具要动五层的测试" |
| 不提供 `ProcessEnv` 的构造器 | `ProcessEnv` 住在 `app`，而邻接表禁止 `testkit → app`。它的构造器留在 `app/tests/` 自己的 `mod common` 里——那里只有一个消费者，也只该有一个 |
| 不提供断言宏 | `assert!(capture.position(..) < .., "{}", capture.summary())` 已经够读。再包一层宏只会让失败消息多绕一跳，而失败消息是夹具唯一真正被人读的输出 |
| 不提供"保留临时目录以便排查"的开关 | 门禁会反复跑，留垃圾会拖垮下一轮。想留就在断言消息里把路径打出来，那比一个谁都记不住要关的全局开关安全 |
| 自己的库代码里不 `unwrap` / `expect` | 根 `clippy.toml` 的 `allow-*-in-tests` 放行的是 `#[cfg(test)]` 与 `tests/` 里的代码；夹具的**非测试**代码不在豁免范围内，所以它一律返回 `io::Result`，由调用方去 `.expect()` |

## 模块

| 模块 | 内容 |
| --- | --- |
| `log` | 全局 subscriber、捕获期互斥、按事件名查询的 `LogCapture` |
| `fixtures` | 假安装根 `TempInstallRoot`、临时库路径 `TempDb` |

两个模块都是私有的，公共出口全在 crate 根上一次性 `pub use`。

## 公共出口

这份清单与源码里的公共项**两侧集合相等**，由 `make check` 的 `test` 门禁比对（实现在 `testkit/tests/discipline.rs`）。
下面那对 `exports:begin` / `exports:end` 注释是给门禁读的，一行一个名字。

<!-- exports:begin -->

- `CapturedEvent`
- `InstallError`
- `LogCapture`
- `TempDb`
- `TempInstallRoot`
- `install`

<!-- exports:end -->

## 几个不显眼但重要的形状

- **不能用 `tracing::subscriber::with_default` 写。** `tracing` 把「这个调用点有人感兴趣吗」的答案缓存在**进程级**，不是线程级。线程局部的 subscriber 因此会在并行测试下互相污染：A 装了 subscriber 触发的缓存结论，B 没装也照样适用，反之亦然。症状是偶发的——单跑绿、全量跑红，或者反过来。这里换成**一个永远说"感兴趣"的全局 subscriber**，真正的开关下沉到 `on_event` 里的 sink 槽位。

- **`CaptureLayer` 的三个方法一个都不能少。** `register_callsite` / `enabled` / `max_level_hint` 必须一起放行；少一个就退回到"按级别过滤"，而级别过滤的结论同样会进那个进程级缓存。

- **sink 是全局的，于是捕获期互斥。** 线程局部 sink 看起来更省事，但本模板要断言的事件（`task_exit` / `storage_close_result`）由 tokio 的工作线程发出，而线程局部 sink 恰好看不见它们——那样写出来的顺序断言会**永远通过**。代价是做日志断言的用例之间串行；`events_from_other_threads_are_captured` 这条用例专门守住这个取舍，线程局部的实现过不了它。

- **捕获期间别的并行用例的事件也会落进同一个 sink。** 所以断言一律按事件名走（`find` / `position` / `count`），不要断言"总共几条"。

- **断言用的事件名要显式写。** `tracing::info!(name: "shutdown_started", …)` 里的 `name:` 是必须的；不写的话 `tracing` 会生成 `event <文件>:<行号>`——一个随行号漂移的名字，拿它做断言等于把测试绑在行号上。

- **互斥用「布尔 + 条件变量」，不用长期持有的 `MutexGuard`。** 后者会让 `LogCapture` 变成 `!Send`，而且在 `async` 用例里跨 `.await` 持有会被 `clippy::await_holding_lock`（设为 `deny`）直接判红——那条 lint 我们要它继续管用。

- **中毒的锁照常用。** 一条用例 panic 会把它当时持有的锁标记为中毒。这里守的只是一个事件列表，没有"可能处于半更新状态"的不变量；为此让其余用例连坐地 panic，只会把"一条失败"变成"一片失败"，真正出问题的那条反而更难找。

- **`TempInstallRoot` 的目录布局由 `core::paths` 算出来，不在夹具里抄一遍。** 安装根也不是直接取 `TempDir::path()`，而是把假可执行文件的路径喂给 `paths::install_root_from`。抄一遍的版本在"生产改了布局"那天仍然全绿，而全绿的测试是最贵的一种测试。`the_anchor_rule_is_the_production_one` 守的就是这一点。

- **`TempDb` 刻意不创建库文件。** 「不存在时把库建出来」是 `storage::open` 的职责之一，夹具先把文件摆好就等于把那条路径测没了——而那恰恰是首次部署会走的路径。

- **这里的全局可变状态不违反"没有全局可变状态"那条纪律。** 那条纪律管的是运行期的库层与 `app`；一个只在 `cargo test` 里被链接进来的夹具不在射程内。它要替代的东西——每个 crate 各抄一份 callsite 规避——才是真正会出事的那个。

## 测试

```bash
cargo test -p <你的项目名>-testkit
```

用例分两类：`fixtures` 那几条验证布局与清理，`log` 那几条验证捕获本身。后者里 `events_from_other_threads_are_captured` 是回归用例——它对应一个**已经犯过的**设计错误，删掉它，线程局部 sink 会悄悄回来。

其余五层的测试通过 `[dev-dependencies]` 用它：

```toml
[dev-dependencies]
service-testkit = { workspace = true }
```
