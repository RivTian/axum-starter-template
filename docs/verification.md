# 验证记录

`docs/acceptance.md` 回答"什么被证明了"。这一份回答另外三件事：**谁跑的、在什么环境下跑的、
以及什么没跑。**

两份合起来才覆盖全部纪律。单看验收清单会高估覆盖面——它按定义只收得下有用例的那些纪律，
而"没有用例"这件事本身也是结论，必须有人写下来。

---

## 0. 这份文件的写法

X-10 记的是上一版的失败：验证记录把"本次跑过"与"沿用上次"混排在同一张结果表里，读者要
读正文小字才知道哪几行不是本次证据。于是这里每一行都自带标记：

- **本次 2026-09-16** —— 这一行是本次实际执行出来的结果。
- **沿用 &lt;日期&gt;** —— 沿用某一次记录的结果，**必须写出是哪一次**，不写就是无效行。

本文件是第一份验证记录，**没有任何一行是"沿用"**。将来补行的人如果要沿用，请把来源日期
补齐；一条查不到出处的"沿用"，和没有证据是同一件事。

另一条写法上的约束：**本文件不写"应该""预期""按设计"**。跑出来什么写什么；没跑的进第 4 节
和第 5 节，不进第 2、3 节。

---

## 1. 环境

一切结论都只在这一组版本上成立。§11 禁止声称未验证的平台，所以这张表既是环境说明，也是
**结论的适用范围声明**。

| 项 | 值 |
| --- | --- |
| 操作系统 | macOS 26.6.2（arm64），内核 Darwin 25.6.0 |
| rustc | 1.95.0 (59807616e 2026-04-14) |
| cargo | 1.95.0 (f2d3ce0bd 2026-03-21) |
| rustfmt | 1.9.0-stable (59807616e1) |
| clippy | 0.1.95 (59807616e1) |
| cargo-generate | 0.24.0 |
| make | GNU Make 3.81 |
| bash | 3.2.57(1)-release |
| perl | v5.34.1 |
| git | 2.54.0 (Apple Git-157) |

钉子与规模：`rust-toolchain.toml` 的 `channel = "1.95.0"`（与上表 cargo 一致，由 `preflight`
当场复核）；`Cargo.lock` 共 195 个包。

**宿主的 SQLite 与结果无关。** 根 `Cargo.toml` 开着 `sqlite-bundled`，SQLite 的 C 源码由
`libsqlite3-sys` 0.37.0 随 `sqlx-sqlite` 0.9.0 一起编进产物。这台机器上装着系统 sqlite3
3.51.0，但它没有参与任何一次构建——记在这里是为了让"换一台没装 sqlite3 的机器会怎样"这个
问题有个明确答案：不会怎样。

---

## 2. 本次跑过的门禁

### 2.1 模板仓库侧：`make check`（九条，合计 52 项）

| 门禁 | 项数 | 结果 | 标记 |
| --- | --- | --- | --- |
| `preflight` | 5 | 通过 | 本次 2026-09-16 |
| `tooling-test` | 9 | 通过 | 本次 2026-09-16 |
| `acceptance-ids` | 3 | 通过 | 本次 2026-09-16 |
| `gen`（`demo-svc`，经 `--path`） | 6 | 通过 | 本次 2026-09-16 |
| `structure` | 9 | 通过 | 本次 2026-09-16 |
| `project-check` | 5 | 通过 | 本次 2026-09-16 |
| `matrix` | 6 | 通过 | 本次 2026-09-16 |
| `migration-rebuild` | 5 | 通过 | 本次 2026-09-16 |
| `release-probe` | 4 | 通过 | 本次 2026-09-16 |

`make check` 整体退出码 0，收尾行为「模板侧九条门禁全绿」。

### 2.2 `check` 之外的两条

这两条**故意**不在 `check` 里（见根 `Makefile` 的头注释：合进去会让本地那条从几分钟涨到
十几分钟，而一个没人愿意在本地跑的门禁等于只剩 CI 一层）。所以它们要单独跑、单独记。

| 入口 | 项 | 结果 | 标记 |
| --- | --- | --- | --- |
| `make verify` | `cold-target`（`CARGO_TARGET_DIR` 未设置，复制树里没有 `target/`）、`expand-test`（走 `cargo generate --test`）、`random-name`（名字由 cargo-generate 自己给，本趟是 `unable-deer`）、`project-gate`（展开树里跑的确实是它自己的 `make check`） | 4 项全过，退出码 0 | 本次 2026-09-16 |
| `make verify-git` | `git-path`（经 `--git` 生成后跑完整条审计）、`crlf-neutral`（敌意 git 配置下两棵树逐字节相同）、`crlf-negative`（拿掉 `.gitattributes` 后 **89** 个文件变成 CRLF——这一项证明上一项的实验是成立的，不是因为环境本来就不会产生 CRLF）、`absent-via-git` | 4 项全过，退出码 0 | 本次 2026-09-16 |

`crlf-negative` 那 89 个文件是本次实测数字，不是估计。它的作用是把 `crlf-neutral` 从"没观察
到差异"抬到"差异确实会发生，而我们挡住了"。

### 2.3 `make lock`

**本次未跑。** 它是五个入口里唯一会写模板仓库工作树的那个（`scripts/normalize-lock.sh` 重写
`Cargo.lock`），所以它不在 `check` 里，也不该在一次"记录现状"的验证里被顺手执行——那会让本
记录描述的树和被验证的树不是同一棵。

代价写明：本记录**不能**断言 `Cargo.lock` 与根 `Cargo.toml` 处于同步状态。能断言的是比这弱
但确定的一条：上面每一条门禁都是在当前这份 195 包的锁文件上跑绿的。

### 2.4 生成结果侧：`make check`（三条）

在一棵干净展开树（`cargo generate --path … --name e2e-demo`，与门禁用的名字不同）里直接跑：

| 门禁 | 结果 | 标记 |
| --- | --- | --- |
| `fmt` | 通过 | 本次 2026-09-16 |
| `lint` | 通过 | 本次 2026-09-16 |
| `test` | 通过（含 `doctest 计数 0` 的复核） | 本次 2026-09-16 |

退出码 0，收尾行为「check 通过：fmt + lint + test 三条全绿」。

注意这一趟与 `project-check` 是**两次独立的执行**：`project-check` 在门禁工作区里跑，共用
缓存、不落 `target/`；这一趟在 `$TMPDIR` 的干净目录里跑，是同一棵刚刚起过进程、服务过真实
HTTP 请求的树。

### 2.5 名字矩阵（极端长短）

`matrix` 本次实跑四组，每组都走 `gen` + `structure` + `env-prefix` + `no-padding`：

| 名字 | 长度 | 结果 |
| --- | --- | --- |
| `x` | 1 | 通过 |
| `tokio` | 5 | 通过 |
| `orders-gateway` | 14 | 通过 |
| `a-very-long-project-name-that-keeps-on-going-there` | 50 | 通过 |

`tokio` 这一组不是凑数：它与一个真实的第三方包同名，用来确认名字替换没有波及锁文件里的
第三方条目。撞名前缀（`--name axum`）的**拒绝**路径由 `tooling-test::prefix-collision` 单独
跑，本次同样绿。

---

## 3. 手工观测（没有任何门禁覆盖的那部分）

下面三段都是这次亲手跑的，日志按原样摘录。日志时间戳是 UTC，本记录的日期是本地日期
（UTC+8），所以 `2026-09-15T22:53Z` 就是 2026-09-16 06:53 本地。

### 3.1 端到端：起进程 → curl → Ctrl-C → 退出码

**标记：本次 2026-09-16。** 干净目录展开 → `make dev-config` → 冷编译 → `cargo run`。

| 观察项 | 结果 |
| --- | --- |
| `GET /healthz` | `200` `{"live":true}` |
| `GET /readyz` | `200` `{"ready":true}` |
| `GET /v1/info` | `200` `{"service":"e2e-demo","version":"0.1.0","uptime_seconds":0}` |
| `GET /v1/nope`（负向） | `404` `{"error":"not_found","message":"no route matches this path","status":404}` |
| SIGINT 之后每个任务都有收尾记录 | 有：`a plane exited task="ticker"`、`a plane exited task="http"` |
| 存储最后关 | 是：`the storage pool was closed` 晚于两条 `a plane exited` |
| 进程退出码 | `0` |

`/v1/info` 里的 `"service":"e2e-demo"` 是名字替换端到端走通的直接证据——它不是模板里写死的
字符串，是展开时填进去的项目名一路走到了 HTTP 响应体。

关停段原样：

```
INFO service_core::lifecycle: lifecycle phase changed from="running" to="draining"
INFO service_app::lifecycle::r#loop: shutdown sequence started phase="draining" plan_invalid=false
INFO service_app::lifecycle::r#loop: a plane exited task="ticker" runtime=1 kind="returned"
INFO service_app::lifecycle::r#loop: a plane exited task="http" runtime=1 kind="returned"
INFO service_app::lifecycle::r#loop: the storage pool was closed outcome="closed"
INFO service_core::lifecycle: lifecycle phase changed from="draining" to="stopped"
INFO service_app: runtime stopped runtime=main budget_ms=24998 elapsed_ms=0
```

两个任务都是 `kind="returned"`——是自己返回的，不是被 abort 的。这是 §12.2「存储最后关」那
一行在**真实信号**下的对照：编排层用例用的是合成停止流，这一趟用的是真的 Ctrl-C。

**一条操作上的坑，写在这里省得下次再踩：`cargo run` 在 unix 上会 exec 替换自己**，所以
`$!` 拿到的就是服务进程本身，`pgrep -P` 找不到任何子进程。第一次跑这条流程时我按"cargo 是
父进程"写了取 pid 的那一步，结果 `kill` 收到空串。要给服务发信号，直接发给 `cargo run` 的
那个 pid 即可——信号不经过中转，这也正是 cargo 这么做的原因。

### 3.2 热重载：SIGHUP 与 `generation`

**标记：本次 2026-09-16。** C-40 把举证责任明确压在这份文件上：`worker` 之所以破例依赖
`tracing`，理由是 tick 是模板里唯一一个"改了配置立刻能观察到效果"的行为，而一个默认看不见
的样板举不了任何证。所以这一段必须真的跑。

做法：起始 `tick_interval = "3s"` → 跑两拍 → 改成 `"1s"` → `kill -HUP` → 继续观察。

```
22:53:13  periodic worker tick seq=1 slept_ms=3000 generation=0
22:53:16  periodic worker tick seq=2 slept_ms=3000 generation=0
22:53:18  config reloaded generation=1 hot=["worker.tick_interval"] semi_pending_plane_restart=[] clamped=[]
22:53:19  periodic worker tick seq=3 slept_ms=3000 generation=0      ← 关键的一行
22:53:20  periodic worker tick seq=4 slept_ms=1000 generation=1
22:53:21  periodic worker tick seq=5 slept_ms=1000 generation=1
```

三条结论，按重要性排：

1. **热改生效**：`seq=4` 起 `slept_ms` 变成 1000，间隔从 3 秒变成 1 秒。重载报告把
   `worker.tick_interval` 列在 `hot` 里，`semi_pending_plane_restart` 与 `clamped` 都是空的
   ——报告说的和实际发生的一致。
2. **`generation` 字段挣到了自己的存在理由**，而且是以最直白的方式：`seq=3` 在重载报告之后
   才打出来，却仍然是 `slept_ms=3000 generation=0`。没有 `generation` 这个字段的话，这一行
   看上去就是"重载报了成功，面还在用旧值"——一个像 bug 的现象。有了它，同一行读出来的是
   "这一拍在重载落地之前就已经读好了自己的间隔，它按老值睡完"，即验收用例
   `a_reloaded_interval_takes_effect_after_the_round_that_already_read_the_old_one` 所声称
   的那条语义，在真实进程里的样子。**这是 C-40 那句"靠它直接看得出来"的实测兑现。**
3. 从 SIGHUP 到报告打出用了不到 0.3 秒；从报告到新值真正被用上，隔了正好一拍。

### 3.3 重载失败：删掉配置文件再 SIGHUP

**标记：本次 2026-09-16。** 这一段验的是反方向。`app/src/config/bootstrap.rs` 的头注释声称：
把写入端放在库里、再让 reload 复用它，后果是"把配置文件删掉再 SIGHUP，进程会静默地把出厂值
写回去"——所以模板用两道结构性约束堵死它。那句声称本身需要证据。

做法：`rm` 掉配置文件 → `kill -HUP` → 看三件事。

```
ERROR service_app::config::reload: config reload failed; the running config is unchanged
      error=cannot read config file `…/target/debug/config/service.toml`
            (selected via default location): No such file or directory (os error 2)
```

| 观察项 | 结果 |
| --- | --- |
| 失败是不是**响的** | 是，`ERROR` 级，且说清了是哪个文件、按什么方式选出来的、底层错误是什么 |
| 配置文件有没有被重新写出来 | **没有**。重载端不写文件，那道结构性约束成立 |
| 进程有没有继续服务 | 有，`/healthz` 仍然 `200` |
| 运行中的配置有没有变 | 没有，后续 tick 仍是 `slept_ms=1000 generation=1` |

四条合起来就是 last-good 语义在真实进程里的形态：**读不到新配置时，旧配置继续有效，并且这
件事是被喊出来的，不是被咽下去的。** 随后一次 SIGINT 收尾干净，退出码 0。

---

## 4. 无自动化证据册

这是本文件最重要的一节。**一条纪律要么在 `docs/acceptance.md` 里带着用例名，要么在这里带着
"为什么没有"。中间没有第三种状态，也不许用测试伪装成已验证。**

第 1～3 条是 DP9 的直接代价（架构 §12.4）。本次的手工观测**触到了**这三条，但手工观测不是
自动化证据：它不会在下一次改动时自动重跑，所以"本次手工"这一列写的是"今天确实看见了"，不
是"以后不会坏"。

| # | 事实 | 为什么没有自动化证据 | 本次手工证据 | 残余风险 |
| --- | --- | --- | --- | --- |
| 1 | `tokio::signal` 真的把 SIGINT / SIGHUP 送进 `os_stop_stream()` | 要向自己发信号，就要 `libc`/`nix` 或外部进程。DP9 裁掉了进程测试层，也裁掉了 Python | **有**：本次收到 2 次真实 SIGINT（两趟都正常关停）、2 次真实 SIGHUP（一次成功重载、一次失败保留 last-good）。SIGTERM **未测** | `signals.rs` 是纯转发：无分支、无状态、无预算计算（D57 强制）。语义全在 `StopPolicy` 里且有用例 |
| 2 | `std::env::current_exe()` 在真实安装布局下返回可执行文件本身 | `ProcessEnv::capture()` 只能在真实进程里取值 | **有**：日志 `install_root=…/target/debug`，而可执行文件在 `target/debug/e2e-demo`，锚点推导正确 | `capture()` 无分支；全部判断在 `install_root_from(exe)` 里且有用例 |
| 3 | 进程退出码被操作系统观察到 | 要有父进程去 `wait` | **有**：两趟 `wait` 都拿到 `0` | `exit_code(&RunReport)` 是纯函数且有用例；`main()` 只做 `ExitCode::from` |
| 4 | `StorageError::VersionConflict` | 模板不预建业务表，这个变体在模板内**没有任何构造路径**。P2 裁定：变体保留以满足归一化契约，HTTP 映射由无 `_` 臂的 `match` 覆盖（新增变体编译不过），但不写伪装成验证的测试 | 无 | 映射本身有编译期保证；构造路径要等第一张业务表 |
| 5 | 半热档配置"下次面重启生效" | 只有重载报告这一侧的证据，没有跨重启的证据——跨重启要的是"停掉再起一遍并比对",那是进程级流程 | 无（本次重载报告里 `semi_pending_plane_restart` 恰好是空的，连样本都没有） | 报告与实际生效之间隔着一次重启，这段没人看着 |
| 6 | symlink 农场下的安装根 | 没有构造这类布局 | **部分**：本次顺带看见一次路径解析——进程 cwd 是 `/var/folders/…`，而日志里的 `install_root` 是 `/private/var/folders/…`。macOS 上 `/var` 是指向 `/private/var` 的符号链接，`current_exe()` 返回的是**解析后**的路径 | 只观察到"单层 symlink 会被解析"这一个点，不构成对 symlink 农场的结论 |
| 7 | `Secret` 的"不回显"在四条单元用例之外的部分 | 四条用例覆盖的是 `Debug` / `Display` / 序列化 / 错误消息四个出口。"任何新写的代码都不会把值打出来"不是能测出来的性质，是要靠类型挡的 | 无 | 靠 `Secret` 不暴露 `Deref`、不实现会泄漏的 trait 来挡；新增出口要人工看 |
| 8 | 进程级线程计数 | 进程内数不准自己的线程数（运行时自己的线程、测试框架的线程混在一起） | 无 | runtime 的线程配置有用例；"实际起了几个线程"没有 |
| 9 | Windows 文件权限，以及一切非 unix 平台 | 没有这些平台的机器 | 无 | §11 禁止声称未验证的平台，README 也不声称 |
| 10 | Linux | 全部实测数字都在 darwin 上取得 | 无 | 门禁脚本按 POSIX + `LC_ALL=C` 写，`grep`/`sed`/`tar` 的 GNU-BSD 差异都被绕开了——但"绕开了"是设计意图，不是证据。CI 矩阵里那一格的**第一次红是信息，不是回归** |
| 11 | `.github/workflows/ci.yml` 整体 | 这台机器上没有 GitHub Actions runner，这份文件**从未被执行过**，每一条 `run:` 的行为都是推断 | 无 | 见下面三条分项 |
| 12 | `cargo install cargo-generate` 能否在钉死的 1.95.0 上编过 | 同上 | 无 | 编不过的话要改成装预编译产物，**不是**放宽模板的工具链钉子——钉子是 `structure` 那堆逐字节比较能成立的前提 |
| 13 | `cargo audit --file` 对"本地包名带占位符"的锁文件的行为 | 同上 | 无 | 模板根 `Cargo.toml` 里是 `{{crate_prefix}}-core`，cargo 解析不了，所以只能让 cargo-audit 直接读锁文件。那个参数存在、也应当能读合法锁文件，但这个具体情形没人试过 |
| 14 | 生成结果的 release 构建 | 生成结果的 `make check` 是 `fmt + lint + test`，不含 release（构建一次 release 太贵，不该压在每次本地门禁上） | 无 | 模板侧的 `release-probe` 用真实 release 产物跑过 4 项（含 `panic="abort"` 负向探针），但那是**模板仓库**的门禁，不是生成结果的 |

第 11～13 条同样写在 `.github/workflows/ci.yml` 的头注释里。两处都写，是因为读 CI 文件的人
和读验证记录的人通常不是同一次进来的。

---

## 5. 怎么重跑

本记录里的每一行都可以重跑。模板仓库根目录下：

```bash
make check
```

```bash
make verify
```

```bash
make verify-git
```

第 3 节那三段手工观测没有对应的 make 目标——它们要起进程、发信号、改配置文件，**故意**不做
成门禁（DP9）。重跑步骤按 §3 各段开头写的做法逐条执行即可；用到的只有 `cargo`、`curl`、
`kill` 和一个文本编辑器，不需要任何额外工具。
