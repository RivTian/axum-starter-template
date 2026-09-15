# {{project-name}}

**受监督 Rust Web 服务起点：五 crate、SQLite、受控热重载与可选 runtime 绑定。**

- 包名前缀：`{{crate_prefix}}`；二进制：`{{crate_name}}`。
- Rust 内部依赖别名固定为 `service_core` / `service_api` / `service_storage` / `service_worker`，不随包名前缀改变源码排版。
- Rust 固定为 `1.97.1`；`make check-process` 的验证脚本需要 POSIX 与 Python 3.11+，bundled SQLite 构建需要 C 编译器。

## 启动、访问与停止

从项目根目录运行：

```sh
make check
cargo run --locked
# 或显式指定配置：
cargo run --locked -- --config /absolute/path/service.toml
```

默认读取 `./config/service.toml`；可通过 `{{env_prefix}}_CONFIG` 指定路径，显式 `--config` 优先。
SQLite 相对路径以配置文件所在目录为基准，不以 cwd 或可执行文件位置为基准。
缺失/非法配置不自动写回；数据目录在受控启动阶段创建。
默认路径与选择逻辑在 app 的 config 模块维护；启动日志用 `config_source=cli/environment/default` 说明来源，不回显配置内容或敏感路径。

`data` 不是独立的全局目录设置：它是 `storage.path` 的父目录。省略该字段也使用 `../data/service.sqlite3` 默认值；
相对路径随配置文件所在目录解析，移动配置文件会改变其含义。没有隐式 `DATA_DIR` 覆盖；需要时在 TOML 中显式引用：

```toml
[storage]
path = "${DATA_DIR:-../data}/service.sqlite3"
```

环境取自启动快照，路径属于冷配置，修改数据库位置必须重启。目录/文件无法创建、连接/迁移/探测失败都中止启动。

开发配置使用 `127.0.0.1:0` 向 OS 申请临时端口，不指定部署端口。
看到 `event="service_started" listen_addr=...` 后，使用日志中的实际地址访问：

- `GET /v1/service/health`：存活，不查询数据库。
- `GET /v1/service/ready`：Running、生命周期发布者存活且数据库有界探测成功才返回 200，否则 503。
- `GET /v1/service/info`：服务名与版本，不暴露配置/数据库路径。

Ctrl-C 或 SIGTERM 会先撤销 readiness、取消根 token，再限时收割任务、关闭存储，最后在同步入口关闭 runtime。
正常收敛返回 0；任务异常、强制 abort、未收割、关闭超时返回非零。
若 HTTP 未取得排空证明，强制路径不执行假定“所有 handler 已停”的正常关池分支。

## 终端日志

日志固定采用 **compact 单行 + 结构化字段**，写入 stdout，由 app 统一初始化；不提供多套日志格式开关。
交互终端自动为日志级别着色、弱化时间戳，并区分字段名与字段值；重定向到文件或管道时不输出 ANSI 样式码。
`NO_COLOR` 为非空值或 `TERM=dumb` 时禁用颜色；空的 `NO_COLOR` 不禁用。颜色和 `RUST_LOG` 过滤均在启动时确定，不热更新。

```sh
NO_COLOR=1 cargo run --locked          # 显式纯文本
cargo run --locked > service.log      # stdout 不是终端，自动纯文本；stderr 仍独立
```

颜色只影响显示，不改变 `event`、任务归属、配置代号与关停结果等字段。
日志采集还应收集 stderr，以覆盖 tracing 初始化前的错误和默认 panic hook 输出。

## 热重载

修改同一配置文件，再向进程发送 SIGHUP（建议原子替换文件）：

```sh
kill -HUP <service-pid>
```

观察 `config_reload` 日志：`published` 是快照提交，`no_change` 不增代，`invalid` / `requires_restart` 不发布。
`ticker_config_applied` 与后续 tick 的 `config_generation` 表示消费者实际观察的版本；它可能跳过中间代号。
不要把 publisher 已提交和消费者已应用视作同一个时刻。

加载在主 runtime 的 blocking 池中进行，最多一个在途作业和一个合并请求。
`lifecycle.reload_timeout_ms` 默认 2000ms，本身是冷字段；到期结果丢弃，但已开始的阻塞工作仍保留槽位直到真实结束。
关停会封住提交入口、取消合并请求并共用原 grace/abort 收割预算；无法收割的 `config_loader` 会列入退出报告，不能被假报完成。
配置路径、环境快照和默认线程数沿用启动时的捕获值，重载不重新定位文件或读取新环境。

## 可选 runtime 绑定

默认配置不创建 extra。确实需要独立调度/线程预算时，在同一配置文件中设置，例如把 ticker 放到一个独立单 worker runtime：

```toml
[ticker]
interval_ms = 1000
runtime = "compute"

[runtime.extra.compute]
worker_threads = 1
max_blocking_threads = 2
```

HTTP 同样可设置 `http.runtime`。不写或写 `"main"` 都使用主 runtime；两面也可共享同一个 extra。
所有 runtime 都用 multi-thread 调度器，单 worker 使用 `worker_threads = 1`，不提供无人驱动的 current-thread 选项。

- extra 名称必须是最多 32 字节的小写 kebab-case，`main` 保留。
- 每个 extra 必须显式提供两个正数线程预算，并被至少一个任务面引用；未知绑定、闲置定义和超预算均在创建线程前拒绝。
- 最多 8 个 extra，worker 总数和 blocking 上限总和分别不超过 256。当前只有两个面，因此未引用的其他定义也会被拒绝。
- runtime 参数及绑定是冷配置；SIGHUP 不迁移任务、不重建监听器或线程池。
- 正常路径先排空任务、关池，再逆序关闭 extra，最后主 runtime；所有 runtime 共享同一个剩余关闭期限。
- 独立的是调度与线程预算，不是 CPU 配额、内存或进程故障的硬隔离。

启动日志会打印 `runtime_topology` 和任务实际线程上下文，退出记录保留任务所属 runtime。
API/worker 不接收 runtime owner 或 executor map；HTTP listener、ticker timer 在各自任务的实际运行上下文创建。

## 验证

```sh
make check          # cargo 门禁：fmt-check / lint / test / build，不需要解释器
make check-process  # 真进程门禁：release / logging / service，需要 POSIX 与 Python 3.11+
# 可单独运行：
make test
make release-check
make logging-check
make service-check
```

两者分开是为了让没有 POSIX 信号或 Python 3.11+ 的环境仍拿得到 cargo 那一半，而不是卡在 preflight 全部失败；CI 两步都跑。不要写成 `make check check-process`，并发目标会同时改动同一个工作区。

系统 Python 低于 3.11 时，使用 `make check-process PYTHON=/absolute/path/to/python3` 指定满足要求的解释器。

`release-check` 构建并运行 `app/tests/fixtures/release_panic.rs`。它复用实际 TaskSupervisor，检查 panic 被观察、兄弟任务完成收割和 runtime 关闭调用返回；夹具故意返回 1，由检查器判定是否符合预期。

这是生成项目自己的回归测试，不是服务使用示例，因此不放在 `app/examples`。它仍随项目交付，CI 不需要回到模板仓库取测试代码。
Cargo 中保留名为 `release-panic-fixture` 的 `[[example]]` 编译目标，是为了使用 `cargo build --release` 的真实 panic 策略：普通 test harness 会忽略 profile 中的 panic 设置，不能用 `cargo test --release` 替代这一证明。
目标设置了 `test = false`、`bench = false`，不由普通测试 harness 执行；`cargo run --locked` 仍只启动服务。
`logging-check` 在 debug/release 的实际进程中验证 PTY 自动彩色、`NO_COLOR`（含空值）、`TERM=dumb`、管道和文件纯文本，且逐例检查正常启动/关停字段。
`service-check` 在 debug/release × 五种布局的实际服务进程中验证端点、信号、启动失败、配置路径/优先级和独立实例。
故障注入只存在于测试/example，生产主程序没有 panic 开关、测试路由或管理后门。
真实服务测试还覆盖未完成 HTTP header 与客户端断开后的有界退出；handler panic 被答复成 JSON 500 且不掀掉连接，由 API 中仅测试路由在真实 socket 上验证。

## 从示例走向自己的服务

### 第一条业务迁移

1. 在 `storage/migrations` 新增单调递增的 `<version>_<description>.sql`，只写首个真实用例需要的 schema。
2. 保持 SQL 的 LF 换行；已应用版本不修改、不删除，schema 变更用新版本追加。无需为模板安装 sqlx-cli。
3. `cargo build --locked` 会重新嵌入迁移集合。启动沿同一 connect → migrate → health 管线完成后才发布存储门面与 readiness。
4. 更新模板的“空业务表/空版本集”示例断言与真实进程 fixture；保留 checksum、错误 SQL、dirty、竞争和中断恢复等失败路径验证，再运行 `make check`。

SQLx SQLite 的 migration advisory lock 不是跨实例排他锁；两个初始化者竞争时，其中一个可能失败。
没有自动重试、自动修复版本元数据或“所有中断都会回滚”的承诺。测试中事务迁移的可重启证据不推广到任意业务 SQL、非事务迁移或断电。

### WAL 与持久性

连接显式以 `journal_mode=WAL`、`synchronous=FULL` 打开，两者都必须显式请求：驱动只为调用方设置过的项发出 pragma。
没有 WAL 时，池中的写者会排他锁住整个数据库并挡住全部读者，多连接池的正常并发会退化成 SQLITE_BUSY。
FULL 在每次提交时 fsync WAL，返回成功的事务能扛住操作系统崩溃和掉电，不只是进程崩溃。代价是每次提交多一次 fsync。
NORMAL 同样不会损坏数据库、还更快，但可能回滚上次 checkpoint 之后已提交的事务；模板不知道你要存什么，默认取持久的一端，要降到 NORMAL 请先实测写入路径。
WAL 是数据库的持久属性，转换需要 busy_timeout 等不到的独占锁，因此它在建立连接选项时设置而不是迁移之后。若已有非 WAL 文件正被其他进程持有，启动在 connect 阶段失败，不会静默降级。
运行时数据目录会多出 `-wal` 与 `-shm` 两个伴随文件，已列入 `.gitignore`。

### 删除 ticker 示例

这是删除代码，不是新增一个 feature/配置开关：

1. 在 app 的 boot 中移除 ticker 的目标解析、注册、启动确认等待；不能仅删除 worker crate 而留下永远等不到的握手。
2. 删除 `BootConfig.ticker_runtime`、RawTicker、拓扑引用校验和示例 TOML 中的 ticker 字段，按新任务面集合更新 runtime 布局测试。
3. 当前 HotConfig 唯一内容就是 ticker 周期。若没有其他真实热配置消费者，一并移除配置 watch、app 重载作业与对应 SIGHUP 分支；不要预留一个空热重载框架。
4. 从 workspace members/dependencies 与 app 依赖中移除 worker；按消费者情况移除 core 中的周期/热配置类型，但保留 HTTP 仍需要的生命周期读端。
5. 使用 Cargo 更新锁文件，替换与 ticker/代号/五布局绑定的示例测试，并重新运行 fmt、Clippy、测试和真实进程关停门禁。

增加新任务时则直接让其 future 返回结果，并由 app 的 TaskSupervisor 持有；不要让任务自己脱离监督去 spawn 一个真正工作的后台循环。

## 当前边界

- 五 crate、SQLite 单后端，无业务表；迁移元数据表不算业务预铺。
- **SIGHUP 只热改 `ticker.interval_ms`**；无变化不增代，非法配置保留旧快照，冷字段或冷热混合变化整批拒绝并要求重启。
- 默认只有一个 multi-thread runtime；可选 extra 与任务绑定只在启动时装配，改动必须重启。
- 任务错误/panic 由顶层监督器处理；请求内部 panic 由 router 的 catch-panic 层就地答复 500，不等于 HTTP 顶层任务退出，也不会送到监督器。
- `abort` 不能强杀不可中断的线程；若要求硬进程截止，交由外部进程管理者实施。
- 已在 macOS arm64 验证模板闭环；附带的 CI 定义覆盖 Linux/macOS，但 workflow 文件存在不等于对应远端运行已通过。Windows 的信号/PTY/进程工具链尚未验收。
- 慢 header、响应 body 流式发送途中的 panic 或强制关停不保证返回 JSON；外层 HTTP task 被 abort 不是逐连接收割证明。
- Rust/MSRV 是当前已验证基线，不声称是理论最低版本；线程/超时预算不是生产 SLA。

增加业务时按垂直切片修改：新增迁移、仓储门面方法、组件逻辑及契约测试；不要提前添加闲置 AppState 字段或通用 Service/plugin 框架。
第一条业务迁移加入时，将模板的“空迁移集”测试改为该业务 schema 的契约，不能靠忽略迁移错误通过启动。
