# axum-starter-template

一个 `cargo-generate` 模板。展开之后得到的是一个 Rust 后端服务骨架：axum + tokio + SQLite，
六个 crate 单向分层，一条从进程启动到干净关停的完整路径，业务逻辑留空。

**这个仓库本身不是那个服务。** 它的根 `Cargo.toml` 里写着 `{{crate_prefix}}-core`，
cargo 连清单都读不完——在这里敲 `cargo build` 只会得到一段解析错误。这不是缺陷，是模板的
定义：名字是展开时才有的东西。

---

## 用它生成一个项目

```bash
cargo generate --git https://github.com/RivTian/axum-starter-template.git --name my-service
cd my-service
make dev-config && make run
```

生成结果自带一份 README，讲的是那个项目怎么用；这一份讲的是**模板本身**怎么改。

前置条件只有三样：Rust 工具链、`cargo-generate`（`>=0.24, <0.25`，区间声明在
`cargo-generate.toml`）、`git`。没有 Python、没有 `libc`、没有任何进程派生——
`make preflight` 会当场检查并在缺东西时告诉你装什么。

项目名有一条会被**当场拒绝**的规则：它的 crate 前缀不能和生成结果依赖的第三方包撞名。
目前的撞名集合是 `axum`、`futures`、`sqlx`、`sqlx-macros`、`tracing`。拒绝发生在
`hooks/pre.rhai` 里，理由和它为什么不能只是警告，写在那个文件的头注释里。

---

## 两份 Makefile，两套门禁

这是模板仓库唯一真正容易搞混的地方，所以放在最前面：

| | 文件 | 跑什么 | 验什么 |
| --- | --- | --- | --- |
| 模板侧 | `Makefile` | shell 脚本（`scripts/`） | **模板展开出来的东西对不对** |
| 项目侧 | `Makefile.project`（生成时改名为 `Makefile`） | cargo | **那个工程本身对不对** |

两者合不成一份：模板根上跑不了 cargo（见开头），生成结果里也不该有 shell 门禁——
F5 要求生成结果的门禁定义**只有**它自己的 `make check`（`fmt` + `lint` + `test` 三条）。

改模板的日常循环是：

```bash
make check     # 模板侧九条门禁，改完就跑这一条
```

只在**依赖变了**的时候多一步：

```bash
make lock      # 刷新模板自己的 Cargo.lock
```

`make lock` 是整个仓库里**唯一一个会往工作树写文件的目标**，也因此永远不在 `check` 里——
一个会顺手改你代码的检查，会让"门禁绿了"和"我的改动被改过了"混成一件事。

---

## 门禁地图

`make check` 是模板侧全部门禁的并集，九条，由便宜到贵排列（环境坏了的时候红在第一条，
而不是三分钟以后）：

| 子目标 | 它逮什么 |
| --- | --- |
| `preflight` | 工具链 / `cargo-generate` 版本 / `git` 不在或不对版本，给一行明确的话而不是一段 cargo 用法提示 |
| `tooling-test` | **门禁自己的单元测试**（9 项）。每条判据喂上合成输入，两个方向都验：该逮住的逮住，该放过的放过 |
| `acceptance-ids` | `docs/acceptance.md` 的 ID 集合与源码里的用例名集合必须**完全相等**——两个方向都查。验收 ID 就是函数名，改名即改 ID |
| `gen` | 树外生成一个默认工程，并当场审计：未展开的占位符、设计稿痕迹、指向未交付文件的引用、随交付文件的语言 |
| `structure` | 分层邻接表逐格相等、第三方依赖的定位、单一可执行入口、sqlx 的 feature 闭包、`ignore` ↔ `assert_absent` 交叉比对、非渲染文件逐字节相等、生成树垃圾扫描 |
| `project-check` | 在生成树里跑它自己的 `make check`，并读日志证明三条**真的都执行了**——退出码 0 证明不了这件事 |
| `matrix` | 四组名字（带连字符 / 50 字符 / 单字符 / 与知名 crate 同名但不撞锁）各走一遍 `gen` + `structure` |
| `migration-rebuild` | 迁移增 / 改 / 删各证明一次强制重编译，外加一条基线和一条负向对照 |
| `release-probe` | 真实 release 产物里任务 panic 的可观察性，外加一条 `panic="abort"` 时编译期守卫必须红的负向对照 |

另有两条**故意留在 `check` 外面**的独立入口，CI 里各占一格：

| 入口 | 它逮什么 | 为什么不在 `check` 里 |
| --- | --- | --- |
| `make verify` | cargo-generate 的 `--test` 展开路径 + **不复用编译缓存**的冷编译 + 一个不是我们挑的随机项目名 | 冷编整棵依赖树。合进去会让本地那条从几分钟涨到十几分钟，而一个没人愿意在本地跑的门禁只剩 CI 一层 |
| `make verify-git` | 真实的 `--git` 安装路径；`.gitattributes` 的换行符中立性（带负向对照）；在克隆出来的树上再跑一遍结构检查 | 同上，它要连着生成三棵树 |

这个安排有代价：本地只跑 `check` 的人碰不到那两条能逮住的问题。代价的接手方写明在
`.github/workflows/ci.yml` 里——那两格存在的全部理由就是接这个代价。

其余两个目标：`make clean` 删掉门禁工作区（`$TMPDIR/axum-template-gate`），
`make help` 是默认目标。

### 门禁自己会不会坏

会，而且坏掉的样子和通过一模一样——一条判据写死成"永远匹配不上"，输出里看到的是一个绿勾。
这在本模板的开发过程中真实发生过**两次**（`docs/architecture.md` 的 C-151 与 C-154）：
一次是多字节字符类在 `LC_ALL=C` 下永远匹配不上，一次是验收 ID 的抽取器漏掉了 12 条带参数的
`#[tokio::test(…)]`——而且后者两侧**仍然相等**，因为文档清单也是拿同一个抽取器建的。
第二次更值得记住：门禁不但绿，还绿得有理有据。

`tooling-test` 就是为这件事存在的：它给每条判据喂合成输入，要求它在该红的输入上**确实红**。
配套的纪律是判据本身只在 `scripts/audit-rules.sh` 里定义一次，`gen.sh` 与 `tooling-test.sh`
共用同一份——不是各留一份副本，因为副本只会朝"永远绿"的方向漂。

它验的是**判据**，不是**接线**。"这条判据有没有被主流程调用"由 `gen.sh` 在真实的生成树上
回答。两者各管一半，缺哪一半都能造出一个全绿而无效的门禁。

---

## 仓库里有什么

进生成结果的：

```
Cargo.toml  core/  storage/  worker/  api/  app/  testkit/
clippy.toml  .gitignore  Cargo.lock
README.project.md  →  生成时改名为 README.md
Makefile.project   →  生成时改名为 Makefile
```

**不**进生成结果的（清单与逐条理由在 `cargo-generate.toml` 的 `ignore` 里；
`hooks/post.rhai` 会在用户侧复核这些确实不在）：

```
docs/  scripts/  Makefile  README.md  rust-toolchain.toml  .gitattributes  .github/
```

`hooks/` 两个文件不在那份清单里，也不能在——cargo-generate 的顺序是
pre-hooks → 按 `ignore` 删文件 → 渲染 → post-hooks，写进去会让 `post.rhai` 在轮到它之前
就被删掉。理由和它的替代方案写在 `cargo-generate.toml` 的末尾。

### 渲染面

`.rs` 文件**一个都不进渲染面**。项目名靠根清单的 `package =` 别名和各成员的 `[lib] name`
挡在 Rust 源之外，于是源码在生成前后逐字节相同，rustfmt 的结果与项目名无关。
白名单（不是黑名单）、以及这三条判据各自的代价，写在 `cargo-generate.toml` 开头。

---

## 文档

| 文件 | 内容 |
| --- | --- |
| `docs/architecture.md` | 设计稿。九个架构主题、十三个决策点，每条纪律配一条理由、每条理由配一个落点。改模板之前先读它 |
| `docs/audit-0913-0915.md` | 对两个备份分支的取证审计。每条结论是「文件路径 + 结论」，不带行号 |
| `docs/study-barter-rs.md` | 对 `barter-rs` 的针对性学习记录：哪些结论被采纳、哪些被否掉、各自的理由 |
| `docs/references.md` | 每一条**不属于本仓库**的事实，连同版本号和出处（探针 / 源码 / 门禁）。末节列出**没有**被验证过的东西 |
| `docs/acceptance.md` | 每条纪律对应哪几条用例。ID 就是用例名，由 `make acceptance-ids` 逐条核对——它是唯一一份**门禁盯着的**文档 |
| `docs/verification.md` | 验证记录：哪些门禁在什么环境下真跑过、结果是什么，以及**没跑过什么**。每一行自带「本次 / 沿用」标记；第 4 节是十四条无自动化证据的事实 |

各 crate 目录下另有一份 README，讲那一层的职责边界和它为什么只能依赖它现在依赖的那些。
那些 README **随交付**——它们在渲染面里，会带着真名进生成结果。

---

## 改模板

1. 改代码或文档。
2. `make check`。红了先读失败信息——每条断言的失败文本都写明了"这条纪律没了会发生什么"，
   而不只是"期望 X 实际 Y"。
3. 动过依赖就 `make lock`，把 `Cargo.lock` 一起提交（生成结果的门禁跑 `--locked`）。
4. 动过展开路径、`hooks/`、`.gitattributes` 或 `cargo-generate.toml` 的，
   额外跑 `make verify` 与 `make verify-git`。

加一条新门禁的方式是挂到 `check` 的依赖链上，不是新开一个需要人记得单独跑的目标——
一个需要人记得的门禁，可以在半年里一次都没执行过而 CI 全程是绿的。
