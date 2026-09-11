# axum-starter-template

一个可复用的 Rust Web 服务模板：多 crate 单向分层 + 一个多线程 Tokio runtime + 受监督的顶层任务面，
带可选的多 runtime 绑定与 `Arc<dyn Storage>` 双后端存储门面。设计文档在
[docs/architecture.md](docs/architecture.md)，
其中的 [纪律清单](docs/architecture.md#2-纪律清单) 是模板的宪法。

## 用模板起一个新项目

前置：cargo-generate ≥ 0.24.0（`cargo-generate.toml` 里的 `cargo_generate_version` 钉死了）。

```bash
cargo install cargo-generate
```

```bash
cargo generate --git <本仓库地址> --name my-service
```

会问一个问题 `crate_prefix`。**两个名字分开**，是「项目叫 `acme-api`、包叫 `acme-*`」的形态：

| 你给的         | 怎么给               | 例子       | 决定什么                                                          |
| -------------- | -------------------- | ---------- | ----------------------------------------------------------------- |
| `project-name` | `--name`，不给就问   | `acme-api` | 仓库名、README 标题                                               |
| `crate_prefix` | `--define`，不给就问 | `acme`     | workspace 成员的包名 `acme-core` / `acme-storage` / `acme-app` 等 |

`crate_prefix` 带正则校验 `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`：小写字母开头，只许小写字母、数字与连字符。

另外三个变量由 [hooks/pre.rhai](hooks/pre.rhai) 推出来，**不会问你**——它们是前两个名字的确定性函数，多问一次就多一次答错的机会：

| 变量                 | 推导                                     | `acme-api` + `acme` 的结果 | 用在哪                            |
| -------------------- | ---------------------------------------- | -------------------------- | --------------------------------- |
| `crate_name`         | cargo-generate 内置，project-name 的蛇形 | `acme_api`                 | 二进制名                          |
| `crate_prefix_snake` | crate_prefix 的 `-` 换成 `_`             | `acme`                     | 代码里的 `acme_core::`            |
| `env_prefix`         | crate_name 大写                          | `ACME_API`                 | 环境变量前缀 `ACME_API_CONFIG` 等 |

### 非交互生成

CI 或脚本里把两个名字都给全，`--silent` 让缺值直接报错，而不是卡在一个没人看的提问上：

```bash
cargo generate --git <本仓库地址> --name my-service --define crate_prefix=my --silent
```

改完模板想立刻试一把时从本地路径生成，不经 git：

```bash
cargo generate --path <本仓库路径> --name my-service --define crate_prefix=my
```

### 常用旗标

| 旗标                                | 作用                                                                                                                            |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `--name <s>`                        | 给出 project-name，跳过提问。不是 kebab-case 会被自动转成 kebab-case，除非加 `--force`                                          |
| `--define k=v`                      | 给出 placeholder 的值，跳过提问；可重复                                                                                         |
| `--silent`                          | 不提问，缺值即报错（配合 `--define` 用）                                                                                        |
| `--destination <dir>`               | 生成到哪个**父**目录下，项目落在 `<dir>/<name>`（默认当前目录）                                                                 |
| `--init`                            | 就地展开到当前目录，不新建一层子目录                                                                                            |
| `--overwrite`                       | 允许覆盖已存在的文件                                                                                                            |
| `--branch` / `--tag` / `--revision` | 从 git 取哪个 ref                                                                                                               |
| `--values-file <f>`                 | 从文件读 `key=value`（一行一条），`--define` 的批量版                                                                           |
| `--vcs <git\|none>`                 | 生成结果要不要 `git init`；只收这两个值                                                                                         |
| `--test`                            | 把**当前目录**当模板展开，再在结果里执行 `CARGO_GENERATE_TEST_CMD`（缺省 `cargo test`），模板作者自检用；所以要先 `cd` 到模板根 |

两条现成的完整调用在 Makefile 里，可以照抄：[`GENERATE`](Makefile:22) 是开发用的就地展开，[`verify`](Makefile:116) 是 `--test` 的用法。

### 生成之后

[hooks/post.rhai](hooks/post.rhai) 收尾做三件事，所以你拿到的是一个干净项目，不带模板自身的工具：

- `Makefile.project` → `Makefile`
- `README.project.md` → `README.md`
- 删掉 `hooks/`（它不能进 `ignore`：`ignore` 先于 post hook 生效，脚本会先于自己被删）

模板自己的 `docs/`、`scripts/`、`Makefile`、`README.md`、`.github/` 由 `[template] ignore` 挡在外面，不进生成结果。

进目录看 README，第一天该做什么写在那里。

## 维护这个模板

仓库本身**不可直接构建**：源码里是 `{{crate_prefix}}` 这类 cargo-generate 占位符。
所有 cargo 命令都在「生成目录」里跑，Makefile 已经包好：

```bash
make check    # fmt-portable + fmt-check + fmt-matrix + clippy + test（日常门禁）
make run      # 在生成目录里 cargo run
make fmt      # 在生成目录里 cargo fmt，再把结果反向替换回模板源码
make verify   # cargo generate 自己的 --test：换一组名字、空 target，从零展开+编译+测试
make lock     # 依赖有变时重新生成 Cargo.lock 并带占位符写回
```

改模板源码时的两条规则：

- 不要在源码里写 `{{` 或 `{%`（Liquid 语法），格式串需要转义花括号的文件加进
  `cargo-generate.toml` 的 `exclude`；
- 引用兄弟 crate 一律写 `{{crate_prefix_snake}}_core::...`，包名写 `{{crate_prefix}}-core`。

### CI

`.github/workflows/ci.yml`，push 到 `main` 与对 `main` 的 PR 触发，两个 job：

| job      | 跑什么        | 说明                                                                                    |
| -------- | ------------- | --------------------------------------------------------------------------------------- |
| `check`  | `make check`  | 五条门禁。带一个 PostgreSQL service container；否则 storage 的 PG 契约用例整组静默跳过  |
| `verify` | `make verify` | 换一组名字、从零编译。刻意不缓存 `target/`，也不挂 PG（它验的是展开路径，不是后端覆盖） |

PG 的门控变量名（`TPL_PROJECT_TEST_PG_*`）由 workflow 里的 `GEN_NAME` 按 `hooks/pre.rhai`
的规则推出，推完有一步拿它和生成结果里的 `PG_ENV_PREFIX` 常量对字面量——推错了的后果是
用例静默跳过而门禁全绿，「没跑过」和「全绿」长得一模一样，只能靠这一步把它变红。

这份 workflow 只属于模板仓库，`.github` 在 `cargo-generate.toml` 的 `ignore` 里，不进生成
结果：生成项目的 `make check` 是三条门禁，没有 `gen` / `verify` 这两个目标。

### rustfmt 不是名字无关的

这是模板**独有**的一类缺陷，普通仓库里不存在，值得单独说。

模板源码没法 rustfmt（里面是 `{{...}}`），格式化只发生在生成目录里。而 rustfmt 的
排版取决于**替换之后**的行宽与排序——于是「对 `tpl-project` / `tplx` 是 fmt-clean」
并不蕴含「对任意名字都是」。两种翻法都真实踩到过：

- **宽度**：`max_width = 100`，以及 `fn_call_width` / `chain_width` 这些 60 列的子阈值。
  一行在短名字下刚好不到阈值、在长名字下越过，rustfmt 就换一种排法，而模板里存的
  是前一种。`assert!(matches!(app, <prefix>_core::AppError::Storage(_)))` 栽在 60 那档。
- **排序**：`reorder_imports` 在每个空行分隔的 `use` 组内排序。同一组里既有 `std` 又有
  `<prefix>_core` 时，`acme_core` 排在 `std` 前、`tplx_core` 排在 `std` 后。

写模板源码时照三条做：

1. **项目名不进表达式**，绑到 `const` / `let` 上再用：长字符串字面量 rustfmt 拆不动
   （`format_strings` 默认关），常量声明本身也短。服务名用 `core::SERVICE_NAME`，
   别再写一遍字面量。
2. **workspace 依赖的 `use` 单独一组**，空行与 std / 第三方 / `crate` / `super` 隔开。
3. **兄弟 crate 的路径写进 `use`，不要在表达式里全限定**——全限定路径把名字带进了
   表达式宽度里，正是第 1 条要躲的。

三条都有门禁兜底：`make fmt-portable` 静态扫描（建模 100 列与 import 分组），
`make fmt-matrix` 换四组名字真跑一遍 rustfmt（覆盖静态扫描建模不了的 60 列子阈值，
名单里刻意留了一组极端长的名字）。两条都在 `make check` 里，加起来一秒出头。

外部仓库的调研取证在 [docs/references.md](docs/references.md)。
