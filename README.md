# axum-starter-template

`cargo generate` 模板：生成出来的项目是一个能起、能停、能测的 Rust Web 服务骨架，
并且把一组难写对的进程纪律（依赖分层、任务监督、关停预算、配置锚点与热重载）写进了代码与注释里。

**这个仓库不是可编译的 Rust 项目**：模板源码里带着 `{{占位符}}`，只有生成之后才是完整项目。
`.rs` 文件也会过 Liquid 渲染（原因见下面的「维护这个模板」）。

## 生成一个项目

```sh
cargo generate --git https://github.com/RivTian/axum-starter-template.git --name my-service
```

只问一个问题：

- `crate_prefix`：包名前缀，例如 `svc` → `svc-core` / `svc-storage` / `svc-app`（`my-service` 是项目名，
  二进制名与 README 标题由它派生，两者互相独立）。

生成之后：`cd my-service`，改 `[workspace.package]` 的 `version` / `authors`，
然后 `make check`、`cargo run`、`Ctrl-C`。接下来照生成的 `README.md` 走：
「切片一：加第一个顶层任务面」「切片二：加一个仓储」。

## 仓库结构

```text
Cargo.toml            渲染成生成结果的 workspace 根（成员、依赖表、每条依赖的理由）
cargo-generate.toml   唯一提示 crate_prefix、ignore 名单、hooks 声明、版本兼容区间
hooks/pre.rhai        派生 crate_prefix_snake / env_prefix，校验两个输入的形状
hooks/post.rhai       生成期文件级断言（必需文件在、模板材料不在），收走 hooks/ 空目录
crates/               六个成员 crate（生成结果的主体；模板侧与生成侧共用）
config-template.toml  内嵌默认配置模板（缺配置文件时落盘，与 Default 逐字段一致）
Makefile.liquid       → 生成结果的 Makefile（fmt-check / lint / test）
README.md.liquid      → 生成结果的 README（含两条垂直切片）
.gitignore.liquid     → 生成结果的 .gitignore
Makefile              模板仓库自己的门禁（见下）
README.md             本文件（模板仓库自己的说明）
scripts/              模板侧证据脚本（矩阵、探针、切片、格式化回写）；不进生成结果
tools/audit/          模板侧审计工具（独立 workspace）；不进生成结果
docs/                 设计文档与验收记录；不进生成结果
```

`ignore = ["docs", "scripts", "tools"]` 之外的东西都会进生成结果——所以顶层目录是被审计的
（`make audit` 有白名单，出现计划外的条目就红）。

## 模板门禁

```sh
make gate        # 全量：audit → slices-check → template-fmt → matrix → check-gen → probe → slices
make audit       # 静态审计：顶层白名单、垃圾文件、符号链接、ignore 路径存在性、占位符合法性
make matrix      # 前缀形状夹具（合法/非法各若干）+ 名字矩阵的 fmt-check（快）
make check-gen   # 极端长名 / 极端短名各一次完整 make check + 生成结果内容审计（慢）
make probe       # 真实进程：构建串、配置落锚点、SIGINT 关停日志、退出码 0
make slices      # 把 README 的两条切片照抄进生成项目并跑通
```

`make gate` 里每个生成项目都在仓库树外、各自用自己的 target（不复用编译缓存）；
需要 `cargo-generate 0.24.x`，生成结果的 `make check` 只需要 make + cargo。

## 维护这个模板

- **模板源码不能直接 `cargo fmt`**（带占位符不是合法 Rust）。改完 `.rs` 之后跑
  `scripts/format-template.sh`：它真实生成一份（固定 `svc` / `dev_service`），在副本上跑 rustfmt，
  再按占位符反向映射回模板源码。
- **渲染面**：所有会进生成结果的文件都过 Liquid。模板里出现 `{{`/`{%`/`{#` 都会被 `make audit` 拦下来
  （cargo-generate 遇到语法错误会直接失败，但那时已经晚了一步）。要么写已知占位符，要么改写代码避开。
- **改名字相关的代码要小心 rustfmt 的名字相关性**：`.rs` 过 Liquid 意味着代码里带用户前缀；
  模板作者要避免把名字放进"会因宽度换行"的构造里（多条目 `use a::{b, c}` 一律写成一条一行），
  并由 `make matrix` / `make check-gen` 用**上限长度前缀**（32 字符）验证格式仍然干净。
- **模板侧与生成侧同名文件**（`Makefile` / `README.md` / `.gitignore`）用 `.liquid` 后缀机制：
  模板侧保留普通名字，生成侧写 `X.liquid`，生成时去后缀并遮蔽模板侧的同名文件。
  名单由 `make audit` 钉住，别随手加第四个。
- **改 README 里的切片代码块**：`scripts/slices/` 是答案文件，`make slices-check` 会逐字比对；
  改一处必须改另一处。
- **发布新版本**：`cargo-generate.toml` 里的 `cargo_generate_version` 是被实测过的区间，
  升级 cargo-generate 之后先跑 `make gate` 再放宽区间。

## 设计文档

- `docs/architecture.md`：目标/非目标、纪律清单、分层、runtime 与任务面、生命周期、配置管线、
  生成面、测试策略、逐条决策记录。
- `docs/references.md`：第三方语义的版本与出处、本机实测记录。
- `docs/acceptance.md`、`docs/verification.md`：不变量 → 证据索引，以及本次验证记录。
