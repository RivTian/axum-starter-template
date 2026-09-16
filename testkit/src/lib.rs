//! 测试夹具。**dev-only**：没有任何 crate 在 `[dependencies]` 里写它，它进不了发布产物。
//!
//! 这个 crate 存在的唯一理由是「同一段绕坑代码不要抄五份」。里面的两类东西——日志捕获与
//! 临时安装根——都不是"通用工具"，而是本模板的几条纪律需要的取证手段：
//!
//! | 要证明什么 | 靠这里的什么 |
//! | --- | --- |
//! | 停机公告先于任何取消动作 | [`LogCapture::position`] 比较事件下标 |
//! | 存储在所有任务退出之后才关 | [`LogCapture::last_position`] |
//! | 配置解析随安装根走，不随 cwd 走 | [`TempInstallRoot`]，三份不同的锚点 |
//! | 坏文件 / 缺文件 / 坏权限下保留 last-good | [`TempInstallRoot::write_config`] |
//! | `open` 会把不存在的库建出来 | [`TempDb`]，路径给了但文件不建 |
//!
//! 它**只依赖 `core`**（分层邻接表里 `testkit` 那一行只有一个 ✔）。夹具依赖被测层会让
//! "测试基础设施"变成第七层，然后"改一层要动夹具、改夹具要动五层的测试"。
//!
//! # 这里刻意没有什么
//!
//! - **没有 `ProcessEnv` 的构造器。** 把它摆进夹具是很自然的想法，但 `ProcessEnv` 住在
//!   `app`，而邻接表禁止 `testkit → app`。它的构造器因此留在 `app/tests/` 自己的
//!   `mod common` 里——那里只有一个消费者，也只该有一个。
//! - **没有断言宏。** `assert!(capture.position(..) < .., "{}", capture.summary())` 已经够读，
//!   再包一层宏只会让失败消息多绕一跳。

mod fixtures;
mod log;

pub use fixtures::{TempDb, TempInstallRoot};
pub use log::{CapturedEvent, InstallError, LogCapture, install};
