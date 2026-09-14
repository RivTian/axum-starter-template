# storage

## 边界

SQLite 单后端；StorageOwner 持初始化/关闭权，Arc<dyn Storage> 仅有 health。

## 目录

src/lib.rs、src/tests.rs、migrations/、build.rs。

## 关键决策

组件参数独立校验；lazy prepare 只保留所有者，连接→迁移→自检成功才发布；关闭由 app 在已确认排空和 deadline 内发起。不暴露 SQLx 类型。

## 测试形态

独立文件库的空迁移/重启/非法元数据、初始化取消、各连接外键选项、持有连接时的有界 close；测试专用 schema 还验证真实版本重跑、checksum/missing/dirty/SQL 错误、双 owner 竞争与 SQLite progress-handler 中断后的重启；不声称这证明了任意业务迁移或断电恢复。
