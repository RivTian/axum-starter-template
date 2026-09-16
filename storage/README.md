# `{{crate_prefix}}-storage`

SQLite 存储实现。门面（`Storage` / `StorageError` / `CloseOutcome`）住在 `core`，这里只放实现。

分家的理由不是整洁，是**可替换性的编译期保证**：`core` 不依赖任何数据库 crate，所以门面签名里
根本放不进 `sqlx` 的类型。换后端的操作因此被定义成「新增一个后端模块、把 `open` 的分派换掉、
更新编译闭包断言」——**门面一行不改**；哪天需要改门面，就说明泄漏已经发生了。

## 它刻意不做的事

- **不打日志。** 一条 `tracing` 事件都不发，`Cargo.toml` 里也没有这条依赖。失败全部通过
  `StorageError` 上抛，由 `app` 在唯一的调用点记录。两层都记的话，同一次失败会在日志里出现
  两次、措辞还不一样——回溯事故时那是最费时间的一类噪声。
  代价是实打实的：`Migration { stage, hint }` 带不出"第几号迁移"，所以 `hint` 一律写成
  **"去哪儿看"**（`migrations/` 目录、`_sqlx_migrations` 表），而不是"出了什么事"。
- **不建 runtime、不起常驻任务。** 这一层只有被调用时才活动；谁来调用是 `app` 的事。
- **不存业务数据。** `0001_baseline.sql` 是空的，迁移集建不出任何业务表——而且有一条用例
  盯着这件事（见下）。
- **不猜形状。** 门面上现在只有 `health()`。第一个业务方法进来时它需要什么参数，现在猜不出来。

## 模块

| 模块 | 内容 | 对外 |
| --- | --- | --- |
| `open` | 唯一入口：建文件 → 连接 → 迁移 → 自检；以及唯一的关闭点 | `StorageOwner` |
| `sqlite` | 池构造、PRAGMA、`Storage` 的 SQLite 实现 | 无 |
| `error_map` | sqlx → `core::StorageError` 的归一化 | 无 |
| `memory` | `InMemoryStorage`，feature 门控，只用 `std` | `InMemoryStorage` |

模块本身全部私有。

## 公共出口

这份清单与源码里的公共项**两侧集合相等**，由 `make check` 的 `test` 门禁比对（实现在 `testkit/tests/discipline.rs`）。
新增一个导出而不改这里就是红的——于是"这个 crate 对外露了什么"永远有人看过。

本 crate 有 feature，所以清单**分两份**：门禁按开关分别比对，不是取并集。取并集的话，
把一个本该门控的名字漏进默认出口就查不出来了——而那正是这两段清单要守住的那条线。

下面两对 `exports:begin` / `exports:end` 注释是给门禁读的，`feature=` 标出这一段属于哪个
开关。它们在渲染后不可见，但改清单的人应该看得见：两段的边界就是"按开关分别比对"这句话的
执行点。

### 默认 feature

<!-- exports:begin feature=default -->

- `StorageOwner`

<!-- exports:end -->

### `test-utils`

在默认清单之上**追加**：

<!-- exports:begin feature=test-utils -->

- `InMemoryStorage`

<!-- exports:end -->

没有池、没有 `Migrator`、没有任何 `sqlx` 类型出现在出口里。

## 谁能构造 `StorageError`

只有这一层。这不是靠纪律，是孤儿规则挡的：`impl From<sqlx::Error> for StorageError` 写不出来
（两个类型对本 crate 都是外部类型），于是归一化只能写成 `error_map` 里的自由函数，而那个模块
是私有的。上层想"顺手造一个存储错误"做不到——这正是想要的。

## `test-utils` 这个 feature

`InMemoryStorage` 是门面可替换性的唯一**肯定式**证据。其余三条约束都是否定式的——"泄漏了会红"，
而一个只有单实现的 trait，无论纪律写得多严，都排除不掉"这个 trait 其实是照着 SQLite 的形状长
出来的"这种可能。

默认关着：编译闭包纪律管的是**默认** feature 下的那一份，这个 feature 关着时那条 feature
集断言一字不变。

代价写在明处：`make check` 必须**额外跑一趟** `--features test-utils`，否则这个实现会在默认
feature 下静默腐烂，而一个永远不编译的实现举不了任何证。工作区级别的调用要写成
`--features {{crate_prefix}}-storage/test-utils`——`--workspace` 之下 cargo 不接受裸 feature 名。

## 几个不显眼但重要的形状

- **双池，写池物理上只有一条连接。** `max_connections(1)`，不是靠约定。SQLite 的写是全库串行的,
  给写池放多条连接只会把冲突从"排队"变成"`SQLITE_BUSY` 重试"。
- **`health()` 探的是读池。** 探活不能和唯一那条写连接抢——否则一次慢写会让 `/readyz` 变红。
- **`create_if_missing(false)`，文件由我们自己建。** 让驱动去建，文件会拿到 umask 给的模式
  （通常 0644），0600 的纪律就被静默绕过了。`open` 自己以 0600 创建，已存在的文件只**收紧**、
  从不放宽。
- **WAL 边车（`-wal` / `-shm`）不需要单独 chmod。** 它们创建时继承主库文件的模式（实测，见
  `the_database_and_both_sidecars_are_owner_only`）。
- **`open` 全程受 `CancellationToken` 约束，且 `biased`。** 启动期收到停止信号要能中断，
  而不是先把迁移跑完再说。中断的错误里带着**停在哪一步**。
- **迁移失败不自动修复。** 校验和不符、上一次迁移中途挂了——一律 fail-fast。在没人看着的时候
  改数据库比启动失败危险得多。
- **`build.rs` 不可删。** 没有它，改 `migrations/` 下的内容不会触发重编译，`sqlx::migrate!`
  嵌的还是上一次的迁移集——测试全绿、跑起来用的是旧 SQL。理由与实测记在
  [`migrations/README.md`](migrations/README.md)。

## 测试

分三层，各自能看到的东西不同，这个分工本身就是断言：

| 位置 | 能看到 | 断言的是 |
| --- | --- | --- |
| `src/*.rs` 的 `mod tests` | 私有字段、私有函数、可以直连原库核对表清单 | 实现细节与第三方语义 |
| `tests/facade.rs` | 只有公共 API | **这个文件用得上 `sqlx` 就说明门面漏了** |
| `--features test-utils` 那一趟 | 多一个后端 | 同一组契约，两个形态完全不同的实现都要过 |

几条值得单独点名的：

- `migration_set_creates_no_business_tables` —— 绕过门面直连原库，断言表清单**恰好**是
  `["_sqlx_migrations"]`。这条用例会自己过期：第一张业务表落地时它必须被改，改它的人就被迫
  重新想一遍"这张表属不属于模板"。
- `a_failed_open_leaves_no_live_pool` —— 证据是**外部可见**的：WAL 边车只在有连接挂着时存在，
  最后一条连接关掉时被删。失败之后它们还在，就说明漏了池。
- `a_modified_migration_fails_fast_and_is_not_repaired` —— 改掉已应用迁移的校验和，断言启动
  失败**且坏校验和原样还在**（没有被"顺手修好"）。
- `close_past_its_deadline_is_reported_as_failure_not_as_closed` —— 攥着一条已借出的连接不放,
  逼 `close` 撞上自己的 deadline。`CloseOutcome` 必须是失败，不能是"关干净了"。
- `both_backends_satisfy_the_same_contract` —— `--features test-utils` 那一趟存在的理由。

本 crate **一条 doctest 都没有**，全模板都没有：doctest 里的 `use` 必须写 crate 真名，而真名由
模板变量展开，写死任何一个都会在别人展开之后失效。示例代码一律标 `text` 围栏——标 `ignore`
仍然会被计成一条 doctest。`cargo test --doc` 的计数必须是 0/0/0。
