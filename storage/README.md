# {{crate_prefix}}-storage

持久化层。对上暴露 `Storage` 门面，对下封装 SQLite / PostgreSQL 双后端。

## 边界

- 依赖方向单向：`storage -> core`。core 永远不依赖本 crate；错误向上转换由本 crate
  的 `impl From<StorageError> for AppError` 完成。
- 公共 API 不出现 sqlx 类型。上层持有 `Arc<dyn Storage>`，拿不到连接池，因而不可能
  绕过门面直接发 SQL。
- 存储层不持有常驻任务。维护动作通过 `run_maintenance` 由上层调度器驱动。
- 模板不带任何仓储与表：`Storage` 只有生命周期四方法。

## 目录

```text
src/
  lib.rs        门面导出（保持窄）
  error.rs      StorageError / StorageResult / 向 AppError 的转换；语义变体是仓储的归一化目标
  model.rs      跨后端通用模型（现在只有 StorageHealth）
  repo.rs       Storage trait；仓储 trait 与访问器随切片进场
  backend/
    mod.rs      init_storage：按配置分发、自检、启动失败指引
    sqlite.rs   双池（writer=1 / 只读池）与 PRAGMA
    postgres.rs 单池、sslmode、密码解析、根因恢复
migrations/
  sqlite/       SQLite 迁移脚本（现在为空）
  postgres/     PostgreSQL 迁移脚本（现在为空）
tests/
  bootstrap.rs          门面形状契约（对象安全、Send+Sync、错误上浮）
  sqlite_bootstrap.rs   SQLite 启动契约（真库、零业务表、关池、权限）
  postgres_bootstrap.rs PostgreSQL 启动契约（需真实 PG，否则跳过；连不通 fail-fast 恒跑）
  support/mod.rs        起库与 PG 门控
```

## 后端关键决策

**SQLite 双池**：writer 池 `max_connections = 1`，把「单写者」从约定变成物理约束；
只读池以 `read_only` 打开，读路径误写会被 SQLite 直接拒绝。两池都显式设
`acquire_timeout = 5s`（sqlx 默认 30s，对单连接写池过长，会让排队成为故障放大器）。

**PostgreSQL** 单池；目标库须已存在（不自动 `CREATE DATABASE`）；连接选项由离散
字段构造而非 DSN 字符串，避免密码随错误信息泄露；建池超时时单独建一条连接取根因。

`init_storage` 是唯一入口：连接 → 迁移 → 健康自检 → 返回门面，任一步失败即 fail-fast。

## 运行测试

```sh
cargo test -p {{crate_prefix}}-storage --all-targets
```

PostgreSQL 契约测试需真实实例，不提供时自动跳过：

```sh
{{env_prefix}}_TEST_PG_HOST=127.0.0.1 \
{{env_prefix}}_TEST_PG_DBNAME={{crate_name}}_test \
{{env_prefix}}_TEST_PG_USER=postgres \
{{env_prefix}}_TEST_PG_PASSWORD=postgres \
cargo test -p {{crate_prefix}}-storage --test postgres_bootstrap
```

声明了 `{{env_prefix}}_TEST_PG_HOST` 却连不上时它们**红而不跳**。

## 增量扩展约束

本 crate 按**垂直切片**扩展，一次只加一个真实业务用例，顺序固定：

1. 明确业务用例（有上层调用方才动手，不做「将来可能要用」的预铺）；
2. 定义该用例的领域模型与行模型；
3. 在 `repo.rs` 定义仓储 trait（方法按用例来，不按表的 CRUD 全集来），并在 `Storage` 上加访问器；
4. **成对**新增 SQLite 与 PostgreSQL 迁移脚本；
5. 抽出两后端共用的 SQL 常量，方言差异处再分叉；
6. 实现两个后端（读走只读池、写走写池；驱动错误归一化为 `StorageError` 的语义变体）；
7. 补双后端契约测试——同一组断言跑两次，一次 SQLite 一次 PostgreSQL；
8. 最后才在上层 crate 接线（api 的 `From<StorageError> for HttpError` 已备好状态码映射）。

硬性规则：

- **绝不允许单后端落地。** 只实现 SQLite 的仓储不予合并。
- **不预建表。** 没有消费者的表就是没人维护的表。
- **不提供假成功实现。** 未实现的能力就不要出现在 trait 上。
