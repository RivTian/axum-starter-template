# {{crate_prefix}}-config

配置：三段入口、路径锚点、唯一加载管线、热度分档与热重载事务。

**不依赖 tokio / tracing / runtime**：这里是纯函数库 + 一个文件监听回调。把配置变成日志级别、
把退避参数交给 supervisor，是装配层的事（`crates/app`）。

## 边界

- 只做"文件 + 环境变量 → 生效配置"这一件事：定位、读取、解析、覆盖、Secret、路径、校验、钳位、分档、回滚。
- 不安装日志、不建任务、不拥有运行时；`watch_file` 只回调"可能变了"，去抖与重载循环属于装配层。
- 不把配置拆成"启动用"和"重载用"两份逻辑：只有一条 [`load`]，重载在它之上做 diff 与回滚。

## 目录

- `src/anchor.rs`：路径锚点（可执行文件所在目录）与 `sqlite:` 相对路径改写。
- `src/source.rs`：三段入口的优先级（CLI > `ENV_PREFIX_CONFIG` > 锚点下的 `config.toml`）。
- `src/pipeline.rs`：唯一管线；缺文件时落盘内嵌模板。
- `src/schema.rs`：文件形态（`FileConfig`，未知键即错误）→ 生效形态（`Config`，类型化 + 已钳位）。
- `src/secret.rs`：`Secret<String>` 与 `url_env > url_file > url` 的解析。
- `src/tier.rs`：热度档位（hot / semi / cold）与叶子路径枚举。
- `src/reload.rs`：重载事务（cold 回滚、报告、last-good）。
- `src/watch.rs`：`notify` 封装（监听配置文件所在目录）。
- `src/env.rs`：环境变量覆盖（`ENV_PREFIX_SECTION__KEY`）与 `EnvSource` 测试接缝。
- `src/embedded.rs` + `config-template.toml`：内嵌默认配置（缺文件时落盘；与 `Default` 逐字段一致，有测试钉住）。
- `tests/pipeline.rs`、`tests/reload.rs`：三段入口、锚点、校验/钳位、Secret 优先级、三档分类与回滚。

## 关键决策

- **锚点不是配置文件所在目录**：CLI/环境变量可以改"配置文件在哪"，但配置内部的相对路径
  （数据目录、`url_file`）永远从可执行文件所在目录派生。"换个目录启动就读到另一份配置/另一份数据"
  这类故障因此结构性消失（`anchor_is_the_only_base_for_relative_paths`）。
- **改动档位以叶子路径登记**：`HOT_PATHS` / `SEMI_PATHS` 是显式白名单，其余必须落在
  `COLD_PREFIXES` 里；三者都不匹配是 `Unknown`，运行期按 cold 处理（回滚 + 报错级事件），
  测试 `every_leaf_path_is_explicitly_classified` 会把"新字段没登记"直接判红。
  这样"漏登记"的方向永远是安全方向，不会静默假报已生效。
- **重置时先构造完整候选**：任何一步失败都返回错误、运行值不变（last-good），不存在半套用的中间态。
- **`Config` 的 `Serialize`/`Deserialize` 是内部接缝**：给重载 diff/回滚用；从文件到配置一律走管线，
  别用反序列化绕开校验与路径派生。
- **钳位而不是拒绝启动**：只拦"继续跑必然失败"的取值（非法 `bind`、0 退避、缺失的 secret 来源），
  其余越界钳到保守值并把 `Notice` 交给装配层打日志——无人值守下拒绝启动的代价更大。
- **`Secret` 不实现 `Display`**：只实现被脱敏的 `Debug`，`expose()` 是唯一拿到明文的入口，
  免得它被 `%value` 顺手打进日志。

## 测试形态

- 单元测试贴在源文件：锚点改写、三段入口优先级、覆盖键白名单、Secret 三档、分类枚举、diff/回滚。
- 集成测试（`tests/`）走真实文件与临时锚点；环境变量一律注入 `MapEnv`，不碰进程环境，
  因此同一进程里并行跑用例也不会互相干扰。
- 暂停时钟无关：config 没有异步逻辑，测试全部是同步的。
