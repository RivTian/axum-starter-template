//! 让 cargo 追踪迁移目录的变更。
//!
//! `sqlx::migrate!` 只对**已存在**的迁移文件生成 `include_str!`，靠它间接获得
//! 重编译追踪。这有一个致命缺口：**新增**或**删除**迁移文件时，宏展开结果里
//! 不含该文件路径，cargo 认不出任何变化，于是跳过重编译——二进制里仍是旧的
//! 迁移集合。
//!
//! 表现极其隐蔽：新写的迁移不报错、不告警，只是**静默不生效**；直到某次无关
//! 改动触发重编译才突然应用。宏内部虽有 `proc_macro::tracked::path` 覆盖这个
//! 缺口，但它要求 nightly，稳定版工具链上拿不到。
//!
//! 声明目录级依赖即可补上：目录 mtime 随增删条目变化，cargo 据此触发重编译。
//! **不要删除本文件。**

fn main() {
    // 目录本身：捕获文件的新增与删除
    println!("cargo:rerun-if-changed=migrations/sqlite");
    println!("cargo:rerun-if-changed=migrations/postgres");
}
