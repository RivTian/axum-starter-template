# {{crate_prefix}}-app

装配层：唯一"知道全部 crate"的地方。CLI/配置入口、资源获取、任务注册、信号、关停编排、退出码。

## 边界

- 库部分（`src/lib.rs`）与二进制入口（`src/main.rs`）分开：`main` 只有五行，
  装配逻辑在 lib 里——集成测试可以构造多套独立装配，不必 spawn 进程。
- 不做业务：生成结果无示例任务面、无 API 端点；根 README 的两条切片展示了"加任务面/加仓储"的落点。
- **零全局态**：`Assembly` 的每个依赖都是参数注入；一个进程里可以同时跑两套（见 `tests/isolation.rs`）。
- 全局只有两样东西，而且都在二进制路径上安装：日志 subscriber（`Telemetry::install`）与 panic hook。

## 目录

- `src/settings.rs`：CLI（`--config`）、路径锚点、三段入口、首载配置。
- `src/assembly.rs`：`prepare` / `register` / `run`——资源获取、两个内建任务面（`http`、`config-watch`）、
  事件桥、提交点、信号监听、关停编排。
- `src/config_state.rs`：生效配置的唯一发布口（可热 / 半热副作用 + watch 视图）。
- `src/config_watch.rs`：`config-watch` 任务面（事件去抖 → 重载事务 → 报告落日志）。
- `src/telemetry.rs`：进程内唯一的 subscriber、退出记录与关停报告的日志落点、panic hook。
- `src/bootstrap.rs`：runtime、看门狗、退出码映射、构建串。
- `build.rs`：编译期注入 git 身份（`BUILD_GIT_SHA` / `BUILD_GIT_DIRTY`）。

## 关键决策

- **提交点**：`Db::open` → `migrate` → `TcpListener::bind` 全部在 `supervisor.start()` 之前；
  任何一步失败都在"没有任务在跑"的状态下返回（退出码 2），不会留下半个进程。
- **信号是控制面，不是任务面**：SIGINT/SIGTERM 的监听任务由 `run` 拥有并在返回前收掉；
  第一次信号推进到 `Draining`，第二次推进到 `Forced`（只加速，不刷新截止）。
- **退出码**：0 干净 / 2 启动失败 / 3 fatal 任务退出 / 4 关停未完成（`exit_code` 有单测覆盖每种组合）。
- **看门狗（L0，15s）**：只存在于二进制路径（库路径不接管进程）；超时打印报告并 `exit(4)`。
  逻辑抽成 `watchdog(...)` 是为了能用虚拟时钟测（`tests/lifecycle.rs`）。
- **构建串**：启动首条日志固定回答"跑的是哪份代码"（版本 + git sha + dirty + profile）。
- **`config-watch` 的去抖方向**：先收到事件、等 250ms、再读文件——编辑器"写临时文件 + rename"的
  两步写入因此不会让我们读到半截配置。

## 测试形态

- `tests/structure.rs`：结构纪律（邻接表、依赖理由、窄门面、零全局态、库不装 subscriber）。
- `tests/lifecycle.rs`：退出码映射、构建串、看门狗、干净关停（每个任务 stopped）、启动失败分类。
- `tests/config_state.rs`：hot/semi/cold 三档应用、非法过滤器拒绝整次重载、watch == 生效值。
- `tests/isolation.rs`：一个进程两套装配并发（生命周期、配置、数据库、取消互不干扰）。
- 需要真实时钟/多线程的用例自己建 runtime（tokio 的 `Runtime` 不能在 async 上下文里 drop）。
