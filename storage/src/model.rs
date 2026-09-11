//! 跨后端通用模型。行模型随各自的仓储切片进场；这里现在只有健康快照。

use serde::Serialize;

/// 存储健康快照。
///
/// 字段刻意保持后端无关：调用方不该因为换了后端就要改判断逻辑。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageHealth {
    /// 后端标识（`"sqlite"` / `"postgres"`），用于日志与诊断展示
    pub backend: &'static str,
    /// 连通性探测是否通过
    pub healthy: bool,
    /// 数据占用字节数；后端无法给出时为 0
    pub db_size_bytes: i64,
    /// 已应用的最大迁移版本号；尚无迁移时为 `None`
    pub migration_version: Option<i64>,
}
