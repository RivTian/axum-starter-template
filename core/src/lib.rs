//! 共享词汇与进程面原语。
//!
//! 这是**叶子层**：它不依赖 workspace 里任何其他成员，其余每一层都依赖它。所以它里面的
//! 每一个类型都要经得起"五个方向同时使用"的检验，而它自己不能知道任何一个方向的存在。
//!
//! # 这里有什么
//!
//! | 模块 | 内容 |
//! | --- | --- |
//! | [`task`] | 任务面：`TaskSupervisor`、`TaskSpec`、`TaskExit`——"谁在跑、怎么退的" |
//! | [`shutdown`] | 关停预算：一个绝对 deadline 沿链传递的那套计算 |
//! | [`lifecycle`] | 生命周期阶段广播：`Starting → Running → Draining → …` |
//! | [`config`] | 配置类型、热度判别、三段式管线的纯函数部分、发布与读取 |
//! | [`storage`] | 存储门面 `trait Storage` 与归一化错误（实现在 `storage` crate） |
//! | [`paths`] | 安装根锚点的纯函数 |
//! | [`build_info`] | 版本号的唯一来源 |
//!
//! # 这里刻意没有什么
//!
//! - **没有 subscriber 初始化**。库 crate 永不装 subscriber：装了就抢占了进程里
//!   唯一一个全局槽位，而被依赖的库无权替进程做这个决定。这里只用 `tracing` 的宏记录事件。
//! - **没有常驻任务**。构造函数不 `spawn`：一个在你不知情时起了后台任务的构造函数，
//!   会让"关停时所有任务都有归属"从第一天起就不成立。
//! - **没有驱动类型**。公共 API 里不出现 `sqlx::`、`axum::`、`tower::`——那会让门面失效。
//! - **没有全局可变状态**。没有 `static mut`、没有 `OnceCell` 单例、没有隐式的
//!   `Handle::current()`。需要什么就从参数传进来。
//!
//! 这四条不是风格偏好：它们各自对应一类真实发生过的失效，每一条的理由写在
//! 本 crate 的 README 里。

pub mod build_info;
pub mod config;
pub mod lifecycle;
pub mod paths;
pub mod shutdown;
pub mod storage;
pub mod task;
