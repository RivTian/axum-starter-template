# 外部语义索引

本设计依赖的每一条**不属于本仓库**的事实，连同它的版本号和出处。

## 怎么读这份文件

§2 的取证纪律要求第三方语义带**版本 + 出处**。理由不是形式：本模板的开发过程里，从
备份分支继承下来的两条"看起来对"的断言实际是假的（`bundled` 的 feature 归属、
tower-http 超时响应的信封覆盖），而它们的表现形式不是门禁漏写，是**门禁写了一条永远
通过的断言**。一条恒真的断言比没有断言更坏——它占着位置，还让人以为这件事被守着。

出处分三类，含义不同，可信度也不同：

| 标记 | 含义 |
| --- | --- |
| **探针** | 写了一段程序跑出来的。结论以「命令 + 实际输出」为准 |
| **源码** | 读的是本机 `~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/` 下的源码 |
| **门禁** | 本仓库有一条门禁天天在验它。这一类最强：版本升级时它会自己红 |

没有"官方文档"这一类。不是文档不可信，是**这份文件里的每一条都不是靠读文档定下来的**——
凡是只读到而没验过的，进的是最后一节。

> 这份文件不是变更日志，也不是教程。它只回答一个问题：**这条断言凭什么是对的。**

---

## 1. 版本基线

本仓库**全部**实测数字的坐标系。换掉其中任何一个，下面的结论都需要重新取证——
这正是 `rust-toolchain.toml` 把工具链钉死的理由（DP6）。

| | 版本 | 来源 |
| --- | --- | --- |
| rustc | 1.95.0 (59807616e 2026-04-14) | `rust-toolchain.toml` 钉死 |
| cargo | 1.95.0 (f2d3ce0bd 2026-03-21) | 同上 |
| cargo-generate | 0.24.0 | `cargo-generate.toml` 声明区间 `>=0.24, <0.25`，`preflight` 复核 |
| git | 2.54.0 (Apple Git-157) | 系统自带 |
| GNU Make | 3.81 | macOS 自带（**没有** `.ONESHELL`，那是 3.82 才有的） |
| bash | 3.2.57 (arm64-apple-darwin25) | macOS 自带 |
| 平台 | darwin（macOS 26.x，Apple Silicon） | **唯一被实测过的平台**，见 §8 |

锁定的第三方版本（`Cargo.lock`，共 195 个包）：

| crate | 锁定版本 | crate | 锁定版本 |
| --- | --- | --- | --- |
| tokio | 1.53.1 | sqlx | 0.9.0 |
| tokio-util | 0.7.19 | libsqlite3-sys | 0.37.0 |
| axum | 0.8.9 | tracing | 0.1.44 |
| axum-core | 0.5.6 | tracing-subscriber | 0.3.23 |
| tower | 0.5.3 | toml | 1.1.6+spec-1.1.0 |
| tower-http | 0.7.1 | serde | 1.0.229 |
| futures-core | 0.3.34 | thiserror | 2.0.20 |

---

## 2. tokio 1.53.1

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `Interval::reset_at` 的语义 | 探针（P-B） | 只移动**下一次** deadline；周期从新点续算。实测：重设后首次 tick 在 53 ms（即重设点），次次 tick 在 552 ms（重设点 + 500 ms 周期） | §5.2 的节拍器 |
| `JoinSet::spawn_on` 的跨 runtime 归属 | 探针（P-B） | 在 A runtime 上对 B 的 `Handle` 调用 `spawn_on`，任务确实在 B 上执行（返回 `Ok("ran-on-other")`） | §5.3 的附加运行期 |
| `JoinSet::try_join_next_with_id` | 探针 | **不是 async**，不消耗超时预算。可以在收割前把已自行结束的任务先收掉 | `core/src/task/supervisor.rs`（C-100） |
| `JoinSet::is_empty()` 的计数口径 | 探针 | 计的是"还没被 join 走的句柄"，**包含已经结束但没被收走的**。拿它判断"还有任务在跑"会得到错误结论 | 同上（C-101） |
| `current_thread` runtime 上 `Handle::spawn` | 探针 | 没有线程对它 `block_on` 时，spawn 进去的任务**永不执行**，也不报错 | 附加运行期只允许多线程形态（§6 坑 8） |
| `Handle::block_on` 的执行线程 | 探针 | future 在**调用线程**上跑，不是在那个 runtime 的 worker 上 | §5.3 |
| `RuntimeMetrics::num_workers()` / `Handle::metrics()` | 探针 | 在 1.53.1 上是 stable，不需要 `tokio_unstable` | 运行期自述 |
| `yield_now()` 在 `Runtime::block_on` 下的可靠性 | 探针 | **不可靠**：它不保证被让出的任务在本次让出后立刻被调度。用它来"确保对方跑到某一步"的测试会间歇性失败 | 测试里改用显式同步原语 |
| `timeout(Duration::ZERO, fut)` | 探针 | 等价于 poll 一次：future 若立刻就绪则成功，否则立即超时 | 用作"不阻塞地试一下" |
| `tokio::signal::unix::signal()` 的调用时机 | 探针（P-G） | 在 runtime **之外**调用会 **panic**，不是返回 `Err` | `run` 收 `FnOnce() -> io::Result<S>` 而非现成的流（C-92） |
| `tokio/rt-multi-thread` 的依赖归属 | 门禁 | 只有 `app` 一个消费者 | `scripts/structure.sh::third-party` |

---

## 3. HTTP 层：axum 0.8.9 / axum-core 0.5.6 / tower 0.5.3 / tower-http 0.7.1

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| 408 的定义（RFC 9110 §15.5.9） | 规范原文 | 408 是"**客户端**没有及时把请求发完"，责任在客户端。handler 自己超时与客户端无关，回 408 是甩锅 | `api/src/error.rs::timeout()` 回 **504**。这是本文件里唯一一条出处是协议规范的断言，也因此 `AUDIT_TRACE_ALLOW_RE` 专门把 `RFC <数字> §` 放行——设计稿痕迹审计会拦下形如 `§15.5.9` 的引用，而这一条是真的规范引用，不是内部小节号 |
| `TimeoutLayer::with_status_code(StatusCode, Duration)` 是否存在 | 源码（`tower-http-0.7.1/src/timeout/service.rs`） | 存在。备份分支 A 声称超时回 408，属实（`new` 的默认值确为 `REQUEST_TIMEOUT`） | — |
| `TimeoutLayer::new` 的状态 | 源码（同上） | 自 tower-http **0.6.7** 起 `#[deprecated]`。在 `-D warnings`（D63）下直接编译失败 | — |
| tower-http 超时响应的形状 | 源码（同上） | 响应体是 `Response::new(B::default())`——**空 body、无 content-type** | ⚠️ 因此**不用**它：它绕过统一错误信封，客户端只能靠状态码猜。改为 `api` 自实现 `handler_timeout`（`api/src/middleware/timeout.rs`）。这两条理由各自独立成立，写在那个文件的头注释里 |
| `axum` / `tower-http` 的依赖归属 | 门禁 | 各只有 `api` 一个消费者 | `scripts/structure.sh::third-party` |

---

## 4. sqlx 0.9.0 / libsqlite3-sys 0.37.0

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `bundled` 是谁的 feature | 探针（P-A） | ⚠️**不是 sqlx 的**，是 `libsqlite3-sys` 的。原设计写错了 | D39 的断言改成**逐包**断言 |
| `--no-default-features` + `sqlite-bundled,runtime-tokio,migrate` 的解析结果 | 探针（P-A）+ 门禁 | sqlx = `_rt-tokio _sqlite derive macros migrate runtime-tokio sqlite-bundled sqlx-macros sqlx-sqlite`；libsqlite3-sys = `bundled bundled_bindings cc pkg-config vcpkg`。依赖图**不含 postgres / mysql / any** | `scripts/structure.sh::sqlx-features` |
| `sqlx::migrate!` 的前提 | 探针（P-E） | 不开 `macros` 时 `sqlx::migrate` **不存在**（E0433）。开了之后可用 | `macros` 是 D37 的硬前提，非可选。代价实测 +6 个 normal 包（122 → 128） |
| 新增迁移文件是否触发重编译 | 门禁（负向对照） | 在 stable 上 `migrate!` **不会**自动跟踪新增的迁移文件——没有 `storage/build.rs` 的话，新加的迁移被**静默忽略** | `storage/build.rs`；`scripts/migration-rebuild.sh::build-rs-load-bearing` 就是拿掉它来证明这条 |
| `sqlx` 的依赖归属 | 门禁 | 只有 `storage` 一个消费者 | `scripts/structure.sh::third-party` |

---

## 5. tracing 0.1.44 / tracing-subscriber 0.3.23

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `ansi` feature 不开的后果 | 探针 | ⚠️ `with_ansi(true)` 在**运行期 panic**，不是编译期报错。暴露方式只有"开发机上跑一次直接崩" | 显式开 `ansi`（C-71） |
| `set_global_default` 的排他性 | 探针 | 每进程一个槽位。第二次调用返回 `AlreadyInstalled` | 编排层场景不能全放进 `app/tests/`（C-108） |
| `EnvFilter::try_new` 对错误指令的宽容度 | 探针 | 比预期宽容：不少写错的指令不会报错，只是不生效 | 配置校验不能依赖它来发现拼写错误 |
| `u128` 的 `Value` 实现 | 探针 | **没有**。`tracing` 的字段里不能直接放 `u128` | 相关字段先转成 `u64` 或字符串 |
| `tracing-subscriber` 的依赖归属 | 门禁 | `app` 与 `testkit` 两个消费者 | `scripts/structure.sh::third-party` |

---

## 6. toml 1.1.6

| 语义 | 出处 | 结论 |
| --- | --- | --- |
| 版本字符串形态 | 锁文件 | `1.1.6+spec-1.1.0`——`+` 之后是它实现的 TOML 规范版本，不是构建元数据的随意取值 |

---

## 7. 工具的语义

### 7.1 cargo 1.95.0

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `resolver = "3"` 的可用性 | 探针（P-F） | workspace 实建通过 | DP6 |
| `env!("CARGO_BIN_NAME")` 的取值 | 探针（P-C） | 在 bin target 内 = `[[bin]].name`，**不是包名**；lib 侧 `option_env!` 取到 `None` | 环境变量前缀只能由 `main.rs` 读出后向下传参（§6.3） |
| `Cargo.lock` 里依赖引用的两种写法 | 探针 | 同名包唯一时写**裸名**（`"tokio"`），有歧义时才写全（`"tokio 1.0.0 (registry+...)"`）。锁文件反向替换必须认这个区别，否则会把第三方的同前缀包一起改掉 | `scripts/normalize-lock.sh`；`scripts/tooling-test.sh::lock-third-party-kept`（A-31） |
| `[profile.release]` 的 `panic` 是否作用于 example target | 探针 | **作用**。所以注入的探针 example 能拿到真实的 release panic 行为 | `scripts/release-probe.sh` |
| 宿主工具的 profile | 探针 | 不传 `--target` 时，build script 等宿主侧产物按**宿主 profile** 编译，不套用 `[profile.release]` | 同上 |
| 多成员 workspace 里的 `--features` | 探针 | 裸的 `--features test-utils` 被拒绝（无法判断属于哪个包），必须写 `<pkg>/test-utils` | `Makefile.project` 的 `STORAGE_FEAT` |
| `--locked` 与 `[workspace.package].version` | 实测 | 改了版本号不 relock，下一次 `--locked` 会红在一个看起来毫不相干的地方 | `Makefile.project` 的 `relock` + README |

### 7.2 cargo-generate 0.24.0

这一节最长，因为模板的正确性**整体**建立在这些语义上，而它们都不在任何规范里。

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `include` / `exclude` 决定什么 | 源码 | 决定**要不要过 Liquid**，不决定进不进生成结果。不在渲染面里的文件是**逐字节拷贝** | `cargo-generate.toml` 开头 |
| `ignore` 决定什么、怎么匹配 | 源码（`src/ignore_me.rs`） | 决定进不进生成结果。实现是 `p.push(dir); p.push(f)` 再按 `Path::exists()` 过滤——**收字面路径，不是通配符**，写错的路径**静默跳过** | 因此 `gen` 要反过来验一遍 `absent` 清单（D52） |
| 无条件删除的三个文件 | 源码（`get_ignored` 的 `default_ignored`） | `cargo-generate.toml` / `.genignore` / `.cargo-ok`。不需要也不应该写进 `ignore` | `cargo-generate.toml` 末尾 |
| hook 文件的生命周期 | 源码（`src/lib.rs` 的 `remove_dir_files(all_hook_files…)`） | 顺序是 pre-hooks → 按 `ignore` 删文件 → 渲染 → post-hooks，**post 跑完之后它自己删掉全部 hook 文件**，只剩一个空的 `hooks/` 目录。所以 `hooks` **不能**进 `ignore`（会让 `post.rhai` 在轮到它之前就被删） | `cargo-generate.toml` 末尾；`hooks/post.rhai` 收尾那段 |
| 目标目录的创建时机 | 门禁 | **在 pre hook 之前**创建。于是 hook 拒绝时会留下一个空目录 | `hooks/pre.rhai` 的拒绝信息里明说了要先删掉它；`scripts/tooling-test.sh::prefix-collision` 验的就是"零文件落盘 + 有这句提示" |
| 是否读模板自己的 `.gitignore` | 门禁（实测抓出来的） | ⚠️**不读**。Finder 留下的 `.DS_Store` 会原样进每一个从本地路径生成的项目——已经发生过一次 | `.DS_Store` 进 `ignore`；子目录里的那些由 `structure` 递归扫（C-138） |
| `--test` 展开的是什么 | 探针（C-152 ①） | 把 `$CWD` 当模板**原地**展开，并在 `$CWD` 里建一个目标目录 | `scripts/verify.sh` 先把模板树复制走再跑 |
| `--test` 下的 `--name` / `--destination` | 探针（C-152 ②） | **都被忽略**。项目名来自它自己的随机词表（实测 `sour-sand` / `inquisitive-arch` / `stingy-request` / `hallowed-growth`） | `scripts/verify.sh::random-name` 把这条变成了覆盖面：每次跑换一个不是我们挑的名字 |
| `CARGO_GENERATE_TEST_CMD` 怎么被执行 | 探针（C-152 ③） | **不过 shell**，按空白切成 argv。所以它只能是「一个程序加参数」 | `scripts/verify.sh` 传的是 `make check` |
| `--test` 日志里的 `Running "…"` 行 | 探针（C-152 ④） | ⚠️**写死的常量**：无论实际执行什么，打的都是 `Running "cargo test" ...`（命令换成 `pwd` 亦然） | 判据改用只有生成结果的 Makefile 才打得出来的三行；`scripts/verify.sh::project-gate` 的注释里写明了不许改回去 |
| Liquid 的 filter 链 | 探针 | `upcase` / `replace` 可链式使用，用于从项目名推导环境变量前缀 | `hooks/pre.rhai` |

### 7.3 git 2.54.0

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `GIT_CONFIG_GLOBAL` 是否影响 cargo-generate 的克隆 | 门禁（C-153） | **影响**。指向一份 `[core] autocrlf = true` 的文件，克隆出来的文本文件会变成 CRLF | `scripts/verify-git.sh::crlf-negative` 靠它制造敌意环境 |
| 命令行 `--gitconfig` 是否影响换行转换 | 探针（C-153） | ⚠️**不影响**。两条都试过——不实测就写，会得到一条永远绿而且永远无意义的断言 | 同上 |
| `.gitattributes` 的 `* -text` | 门禁（带负向对照） | 抑制换行转换。**负向对照实测**：拿掉它之后同一趟有 **89 个文件**变成 CRLF | `scripts/verify-git.sh::crlf-neutral` + `crlf-negative`。没有后者，前者的绿证明不了任何事 |

### 7.4 shell / POSIX 工具（darwin）

这一节是门禁脚本的**可移植性账本**。它之所以存在，是因为其中一条踩下去的代价是
"一条判据死了，而输出和通过一模一样"（C-151）。

| 语义 | 出处 | 结论 | 落点 |
| --- | --- | --- | --- |
| `LC_ALL=C` 下的 `[…]` | 门禁（C-151） | ⚠️ 是**字节**集合。`第[一二三四五六七八九]阶段` 要求两字之间恰好一个字节，而中文数字是三字节——**这条分支一个真实输入都匹配不上**，而且看起来是绿的 | `scripts/lib.sh` 钉 `export LC_ALL=C`；正则里不许出现多字节字符类，中文枚举写成 `第(一\|二\|…)阶段`；`scripts/tooling-test.sh::history-shape` 用 `第三阶段` 这条用例把它钉住 |
| 为什么钉 C 而不是某个 UTF-8 | 推理 + 实测 | cargo 与 git 都按字节序排，且此前全部实测都在 C 下取得——钉 C 是**保持行为**，钉 UTF-8 是**改行为**。顺带把 `sort` / `comm` 的排序规则钉住：多处相等比较靠两边同一套排序，`zh_CN.UTF-8` 下 `comm` 会对着它认为没排好序的输入给出错误差集而**不一定报错** | 同上 |
| 交互 shell 的 `grep` | 实测 | 本机交互 shell 里 `grep` 被一个指向 **ugrep 7.8.4** 的函数遮住，而 ugrep 按字符处理。**同一条正则手敲和在 `make` 下跑出两个结果** | 纪律：**永远不要靠手敲来验证一条门禁正则**。C-151 藏了好几轮就是因为这个 |
| bash 3.2 缺什么 | 实测 | 没有 `mapfile`、没有关联数组、没有 `${x^^}`。另有一条：`local a="$1" b="$a"` 在 `set -u` 下**不成立**，必须拆成两条 `local` | 全部门禁脚本按 3.2 写 |
| `set -e` 与 `&&` 列表 | 实测 | `cmd && die "…"` 在 `cmd` 不匹配（也就是正常那条路）时整个列表退出码为 1，是否退出取决于它在脚本里的位置。改用 `if` | `scripts/project-check.sh::default-goal` 的注释 |
| `grep -c` 的退出码 | 实测 | 零匹配时退出 1。在 `set -e` 下的 `$( )` 赋值里必须补 `|| true` | 各脚本 |
| `grep -n` 的文件名 | 实测 | 只给一个文件时**不打**文件名。要稳定输出必须同时给 `-H` | 各脚本 |
| BSD 工具与 GNU 的差异 | 实测 | BSD `sed` 没有 `+N` 相对范围、BRE 里没有 `\|` 交替（要 `-E`）、`sed -i` 必须写 `sed -i ''`；BSD `grep` 没有 `-P`；`awk` 不支持 `\s`；没有 `timeout(1)`、没有 `flock(1)`、没有 `realpath` | 脚本一律取两者的交集；互斥用 `mkdir`（`scripts/lib.sh::gate_lock`） |
| macOS 的路径 | 实测 | `/var` 是指向 `/private/var` 的符号链接，`$TMPDIR` 在它下面且**带尾斜杠**。祖先判断是纯字符串比较，两边必须先落到同一坐标系 | `scripts/lib.sh::gate_root_physical` |
| GNU Make 3.81 | 实测 | macOS 自带的是 3.81，**没有** `.ONESHELL`（3.82 才有） | 根 `Makefile` 与 `Makefile.project` 都按 3.81 写 |

---

## 8. 没有出处的地方

以下事实**没有**被本仓库验证过。它们要么写在设计里但只能靠推理，要么需要本机拿不到的环境。
逐条的影响与"如果它是假的会怎样"写在 `docs/verification.md`。

- **Linux / Windows / 任何非 darwin 平台。** 模板全部实测数字都在 macOS 26.x / Apple Silicon /
  bash 3.2 / BSD 用户态上取得。门禁脚本是按 POSIX + `LC_ALL=C` 写的，GNU 与 BSD 的差异都被
  刻意绕开了——但"绕开了"是设计意图，不是证据。`.github/workflows/ci.yml` 的矩阵里有
  `ubuntu-latest`，正是因为这件事没有证据。
- **`.github/workflows/ci.yml` 整个文件从未被执行过。** 写它的机器上没有 runner。
  其中三处不确定（Linux、`cargo install cargo-generate` 能否在钉死的 1.95.0 上编过、
  `cargo audit --file` 会不会介意锁文件里带占位符的本地包名）写在那个文件的头注释里。
- **Windows 上的文件权限语义。**
- **`std::env::current_exe()` 的返回值**在各平台上的具体形状。
- **进程被 OS 观察到的退出码**，以及 `tokio::signal` 的实际投递。这两条属于 DP9 划出去的
  进程边界，本仓库不做进程派生（§11.4 / F5：生成结果的依赖面只有 Rust 工具链本身）。
