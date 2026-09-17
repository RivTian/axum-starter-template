//! 存储门面：`Db`（开池 / 迁移 / 关池）。
//!
//! 骨架里**没有任何表、没有任何仓储**：只有"怎么把连接池开对、迁移跑对、关池关对"。
//! 加第一张表和第一个仓储的步骤见生成项目根 README 的切片二。

mod db;
// 你的仓储模块声明与导出在这里（切片二会加 `mod notes;` / `pub use notes::NotesRepo;`）

pub use db::Db;

// 公共 API 里到处是 core::Error，调用方不该为了接一个错误再去加一条直接依赖。
pub use {{crate_prefix_snake}}_core::Error;
pub use {{crate_prefix_snake}}_core::ErrorKind;
