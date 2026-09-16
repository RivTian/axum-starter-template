//! 存储实现层：SQLite。
//!
//! 门面（`Storage` trait、`StorageError`、`CloseOutcome`）住在 `core`，这里只放实现。
//! 分家的理由不是整洁，是**可替换性的编译期保证**：`core` 不依赖任何数据库 crate，所以
//! 门面签名里根本放不进 `sqlx` 的类型。换后端的操作被定义为「新增一个后端模块、把 `open`
//! 的分派换掉、更新编译闭包断言」——**门面一行不改**；如果需要改门面，就说明泄漏已经发生。
//!
//! # 出口只有两个名字
//!
//! [`StorageOwner`]，以及 `test-utils` 打开时的 [`InMemoryStorage`]。
//!
//! 没有池、没有 `Migrator`、没有任何 `sqlx` 类型。这不是靠评审维持的：出口清单与本层
//! README 的「公共出口」一节由 `make check` 的 `structure` 门禁比对，多导出一个名字
//! 而不改文档就是红。
//!
//! # 谁能构造 `StorageError`
//!
//! 只有这一层。孤儿规则挡住了 `impl From<sqlx::Error> for StorageError`（两个类型对本 crate
//! 都是外部类型），于是归一化只能写成 `error_map` 里的自由函数，而那个模块是私有的。
//! 上层想"顺手造一个存储错误"是做不到的——这正是想要的。
//!
//! # 模块
//!
//! | 模块 | 内容 |
//! | --- | --- |
//! | `open` | 唯一入口：建文件 → 连接 → 迁移 → 自检；以及唯一的关闭点 |
//! | `sqlite` | 池构造、PRAGMA、`Storage` 的 SQLite 实现 |
//! | `error_map` | sqlx → `core::StorageError` 的归一化 |
//! | `memory` | `InMemoryStorage`，feature 门控，只用 `std` |
//!
//! 全部私有。
//!
//! # 这一层不打日志
//!
//! 一条 `tracing` 事件都不发，`Cargo.toml` 里也没有这条依赖。失败全部通过 `StorageError`
//! 上抛，由 `app` 在唯一的调用点记录。两层都记的话，同一次失败会在日志里出现两次、措辞还
//! 不一样——回溯事故时那是最费时间的一类噪声。

mod error_map;
mod open;
mod sqlite;

#[cfg(feature = "test-utils")]
mod memory;

pub use open::StorageOwner;

#[cfg(feature = "test-utils")]
pub use memory::InMemoryStorage;
