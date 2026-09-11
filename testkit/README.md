# {{crate_prefix}}-testkit

集成测试共享件。**仅供各 crate 的 dev-dependencies 使用，不进任何发布产物的依赖图**
（`publish = false`）。

## 边界

- 依赖方向：`testkit -> core + storage`。它在依赖图里是旁支，没有任何 crate 的
  `[dependencies]` 里有它。
- **准入线：≥ 2 处测试共用才进来。** 只用一处的助手留在那个测试文件里。
- **不为测试改生产代码。** 派生端口经测试专属配置文件的 `[http].port` 喂给正常装配，
  bind 逻辑一个字不改。
- **core 不引它**：core 是叶子，dev-dependencies 也不例外（testkit 依赖 core，反过来成环）。
  core 与 worker 的测试自己拼临时目录。

## 目录

单文件 `src/lib.rs`，四样东西：

| 导出                | 用途                                                     |
| ------------------- | -------------------------------------------------------- |
| `temp_dir(tag)`     | `<tmp>/<crate_name>-<tag>-<pid>-<nanos>`，一用例一个目录 |
| `temp_storage(tag)` | 临时目录里起一个跑完迁移的 SQLite **文件**库门面         |
| `test_port(offset)` | `12000 + pid % 20000 + offset`                           |
| `LogCapture`        | 按线程收集 tracing 输出的内存缓冲                        |

## 关键决策

**目录名带 pid 与纳秒。** 同一测试二进制内多用例并行、多个测试二进制并行，都不会撞目录。
调用方用完自己删，删失败只留垃圾、不影响正确性。

**端口按 pid 派生 + 用例内偏移。** 同一二进制内多用例共存不互抢；并行的其他测试二进制是
别的进程，自然错开。

**`temp_storage` 用文件库而不是内存库。** 写走 writer 池、读走 reader 池，而 SQLite 内存库
的每条连接各自独立——用内存库会出现「写完读不到」的假绿。

**`LogCapture` 装的是全局 subscriber，不是 `subscriber::with_default`。** 这是本 crate 里最
值得读的一段注释（`install` 函数头）。tracing 的 callsite「兴趣」缓存是**进程级**的：同一个
`info!` 点被另一条没装 subscriber 的线程命中时，缓存会被重建成「没人要」，此刻别的线程上
用 `with_default` 装的线程局部 subscriber 就收不到这条事件。并行测试里这不是理论风险——
实测两百次循环丢 1 条，而当兄弟用例恰好在敲同一批 callsite 时几乎必丢，表现为「日志少了
中间几行」的假失败。

装一个恒定存在、恒定 enabled 的全局 subscriber，兴趣缓存就稳定为「要」；事件一律送到这里，
再按线程局部的 `SINK` 决定进缓冲还是进废纸篓。输出与生产同形（compact、带线程名、
INFO 级），断言照着生产真出的那行写，测试里断得到的生产也看得到；只关掉 ANSI，
否则颜色码会把 `key=value` 拆断。

**`LogCapture` 按线程生效，有一条已知边界：** 跨线程的 async 任务（spawn 到别的 worker 上）
打的日志收不进来。要断言那类日志，断它的同步注册段，或让任务把结论发到 `EventBus`。

## 测试形态

五条同步单测，其中一条是**并发回归测试**：`capture_survives_concurrent_hits_on_the_same_callsite`
起一条噪声线程持续敲同一个 callsite，主线程循环 200 次捕获并要求 `lost == 0`。它守的正是上面
那段决策——换回 `with_default` 这条测试就红。

另一条 `events_outside_capture_are_discarded` 的标记词刻意不含用例名的任何词根：
`with_thread_names` 会把用例名写进每一行，用例名当标记词会自己把自己断成绿。

```sh
cargo test -p {{crate_prefix}}-testkit
```
