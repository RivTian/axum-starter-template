# 验收：不变量 → 证据索引

> 规则：每条不变量给**可重复证据**（仓库相对路径 + 测试名 / 门禁目标 / 场景），不用测试总数替代逐项对应；
> 没有自动化证据的条目显式写"无自动化证据"，不留白。
>
> 证据跑法：生成侧证据 = 在任一生成项目里 `make check`（`cargo test --workspace`）；
> 模板侧证据 = 在模板仓库里 `make gate`（或单个目标）。生成矩阵与切片由模板侧脚本自动完成。

## 0. 证据形态总览

| 形态 | 位置 | 覆盖 |
| --- | --- | --- |
| 生成侧测试（104 条） | `crates/*/src`（单元）、`crates/*/tests`（集成） | I1–I4 与支撑约束的行为面 |
| 生成侧门禁 | 生成项目 `Makefile` → `fmt-check` / `lint` / `test` | I5：门禁唯一定义 |
| 模板树审计 | `tools/audit` → `make audit` | I5：渲染面、占位符、垃圾、ignore、不预建表 |
| 生成结果审计 | `tools/audit` → `make check-gen`（内含） | I5：无模板材料 / 无里程碑词 / 无门禁目标名 |
| 生成矩阵 | `make matrix` + `make check-gen` | I5：前缀夹具、极端长/短名、完整 `make check` |
| 切片走通 | `make slices` | I5：两条 README 垂直切片 |
| 真实进程探针 | `make probe` | I3/I4/I5：构建串、锚点、SIGINT、退出码 |

## 1. I1 单向分层的 workspace，越界即红

| # | 证据 | 位置 |
| --- | --- | --- |
| 1.1 | 实际依赖图 == 邻接表（三张依赖表合并后逐成员比对，多一条少一条都红） | `crates/app/tests/structure.rs::sibling_edges_match_adjacency_table` |
| 1.2 | `core` 是叶子（无任何兄弟依赖） | `crates/app/tests/structure.rs::core_is_a_leaf` |
| 1.3 | 成员依赖一律 `workspace = true`（第三方只在根声明一次） | `crates/app/tests/structure.rs::member_deps_are_workspace_inherited` |
| 1.4 | 根依赖表的每条都被至少一个成员使用，且行内有"为什么"注释 | `crates/app/tests/structure.rs::workspace_deps_have_reasons_and_are_used` |
| 1.5 | `lib.rs` 只做 `mod` + 显式 `pub use`（通配 re-export 需要 `// 例外:` 理由） | `crates/app/tests/structure.rs::lib_facades_are_narrow` |
| 1.6 | 骨架里没有例外出口 | 同上（生成结果中 `// 例外:` 出现 0 次，见 `make audit` 生成结果审计） |
| 1.7 | `sqlx` 编译闭包只用 SQLite（bundled），不需要系统库 | `crates/storage/tests/db.rs::pool_opens_and_migrations_apply`（本机无系统 sqlite 依赖） |

**限制**：结构测试是文本/清单级检查，它证明"登记的边 == 实际的边"，不证明运行期不存在动态耦合
（Rust 里后者不存在，所以这一点不构成风险）。

## 2. I2 顶层任务面可监督

| # | 证据 | 位置 |
| --- | --- | --- |
| 2.1 | 五类退出各有记录：Completed / Failed / Panicked / Cancelled / Restarted | `crates/runtime/tests/supervisor.rs::records_{completed,failed,panicked,cancelled,restarted}_exit_*`（5 条） |
| 2.2 | `cause`（谁的意图）与 `outcome`（future 结局）分离：关停期优雅返回 = `Cancelled + Returned` | `crates/runtime/tests/supervisor.rs::records_cancelled_exit_for_task_cancelled_at_shutdown` |
| 2.3 | 同一 key 任何时刻只有一个化身（峰值并发 == 1，重启 3 次后仍为 1） | `crates/runtime/tests/supervisor.rs::restart_waits_for_previous_incarnation_single_identity` |
| 2.4 | 旧化身拒不走时拒绝重启，且不起第二个化身 | `crates/runtime/tests/supervisor.rs::restart_is_refused_if_previous_incarnation_will_not_stop` |
| 2.5 | 退出记录带任务名 + runtime；广播通道与报告是同一批记录 | `crates/runtime/tests/supervisor.rs::exit_records_are_published_to_subscribers` |
| 2.6 | 任务面返回 future、spawn 目标由装配层给：同一份任务面跑在附加 runtime 上并被收割 | `crates/runtime/tests/supervisor.rs::task_spawned_to_aux_runtime_is_harvested_with_its_runtime_identity` |
| 2.7 | 未注册的 runtime 名字是启动错误（不回落） | `crates/runtime/tests/supervisor.rs::registering_unknown_runtime_is_a_startup_error` |
| 2.8 | 重复 key 注册被拒绝 | `crates/runtime/tests/supervisor.rs::duplicate_key_registration_is_rejected` |
| 2.9 | 空 supervisor 正常退出、不留 detached、活化身计数归零 | `empty_supervisor_stops_cleanly`、`tracked_tasks_all_finished_after_shutdown` |
| 2.10 | 停止之后不再起新化身（重启预算不越过关停边界） | `no_restart_happens_after_stop_requested` |
| 2.11 | fatal 任务自行退出触发进程级关停（`trigger = TaskFailure`） | `fatal_exit_requests_stop_with_task_failure_trigger` |
| 2.12 | 重启预算耗尽进终态；退避指数增长并封顶 | `restart_budget_exhaustion_is_terminal_and_fatal`、`restart_backoff_is_exponential_and_capped` |

**限制**：`Restarted` 的触发点是 `SupervisorHandle::restart(key)`（骨架里没有调用它的业务代码——
它的消费者是测试与 README 记录的用法）。这是 I2 要求五类可观测的直接后果：策略重启发生在旧化身
自行退出之后，无法把旧化身归为 `Restarted`。

## 3. I3 关停可预测、可解释

| # | 证据 | 位置 |
| --- | --- | --- |
| 3.1 | 预算分层（内层之和 < 外层）：`inner_sum() < total`、`forced_phase < harvest` | `crates/runtime/src/shutdown.rs::tests::inner_budget_leaves_margin_under_total` |
| 3.2 | 每段绝对上限：固定预算下关停耗时落在 `[drain+harvest, 各段之和]` | `crates/runtime/tests/shutdown.rs::every_phase_respects_absolute_budget` |
| 3.3 | 二次信号只加速、不刷新截止（2s+4s 的预算下 500ms 内结束，且 `forced=true`） | `crates/runtime/tests/shutdown.rs::second_stop_request_accelerates_without_extending_deadline` |
| 3.4 | 收不回来的任务如实上报（`still_running` + `aborted`，不进 `stopped`） | `crates/runtime/tests/shutdown.rs::shutdown_reports_still_running_task_honestly`（同步阻塞任务：真实时钟 + 双 worker） |
| 3.5 | `stopped` 只收"停止之后在预算内结束"的任务 | `crates/runtime/tests/shutdown.rs::tasks_that_finished_before_stop_are_not_counted_as_stopped` |
| 3.6 | 资源逆注册序关闭、超时如实上报 | `resources_close_in_reverse_registration_order`、`resource_close_is_bounded_and_reported_honestly` |
| 3.7 | 相位单调推进（不允许被拉回） | `crates/runtime/src/shutdown.rs::tests::phases_only_advance` |
| 3.8 | 真实进程：SIGINT → 每个任务 `stopped task=<名> runtime=<rt>` → 退出码 0 | `make probe`（模板侧） |
| 3.9 | 退出码映射：0 / 2 / 3 / 4 四种组合各有断言 | `crates/app/tests/lifecycle.rs::exit_code_mapping_covers_all_cases` |
| 3.10 | L0 看门狗到点触发、被 abort 时不触发 | `crates/app/tests/lifecycle.rs::watchdog_{fires_at_the_deadline,is_silent_when_aborted}` |

**限制**：L0 看门狗在二进制路径上会 `process::exit(4)`，测试验证的是它的时序逻辑
（`watchdog` 函数），不是"进程真的被强杀"；强杀路径没有自动化证据（触发它需要造一个超过 15s 的
关停，代价高且与平台时序耦合）。

## 4. I4 三段式配置加载 + 热重载不骗人

| # | 证据 | 位置 |
| --- | --- | --- |
| 4.1 | 三段入口优先级：CLI > `ENV_PREFIX_CONFIG` > 锚点默认 | `crates/config/tests/pipeline.rs::cli_beats_env_beats_anchor_default`、`crates/config/src/source.rs::tests::{cli_beats_env_beats_anchor_default,empty_env_value_falls_through_to_anchor_default}` |
| 4.2 | 锚点是相对路径的唯一基准（sqlite 相对路径 / `url_file` 都从锚点派生） | `anchor_is_the_only_base_for_relative_paths`、`relative_url_file_is_anchored_and_missing_file_is_an_error`、`crates/config/src/anchor.rs::tests::sqlite_relative_paths_are_anchored` |
| 4.3 | 真实进程：从两个不同 cwd 启动读到同一份配置，配置只在可执行文件旁 | `make probe` |
| 4.4 | 零配置可起：缺文件时落盘内嵌模板 | `missing_config_file_is_written_from_embedded_template` |
| 4.5 | 内嵌模板与 `Default` 逐字段一致 | `embedded_template_matches_schema_defaults_field_by_field` |
| 4.6 | 未知键即错误；缺段等价默认 | `unknown_keys_are_errors_and_missing_sections_are_defaults` |
| 4.7 | 环境变量覆盖（形状 `PREFIX_SECTION__KEY`），拼错键即错误 | `env_value_overlay_applies_and_unknown_prefixed_env_is_rejected`、`crates/config/src/env.rs::tests::{applies_keys_with_double_underscore_only,unknown_key_in_overlay_shape_is_an_error,integer_override_must_parse}` |
| 4.8 | 只拦"继续跑必然失败"的取值；越界钳位 + notice | `values_that_cannot_run_are_rejected_and_out_of_range_is_clamped`、`notices_are_reported_on_reload_too`、`crates/config/src/schema.rs::tests::clamps_out_of_range_with_notice` |
| 4.9 | 敏感信息 `url_env > url_file > 明文`；Debug 渲染 `***` | `secret_precedence_is_env_then_file_then_literal`、`crates/config/src/secret.rs::tests::{env_wins_over_file_and_literal,file_wins_over_literal_and_is_anchored,debug_never_renders_the_value}` |
| 4.10 | 缺失的 secret 来源拒绝启动 | `missing_secret_source_is_a_startup_error` |
| 4.11 | 唯一管线：启动与重载共用 `load_candidate`（同一函数读文件） | 代码落点 `crates/config/src/pipeline.rs::load`；`invalid_reload_keeps_last_good` 与 `deleting_the_file_falls_back_to_the_embedded_defaults` 覆盖两条路径 |
| 4.12 | 冷段漏登记会红：每个叶子必须被显式分类 | `crates/config/tests/reload.rs::every_leaf_path_is_explicitly_classified` |
| 4.13 | 可热立即生效；半热记入 next_use 并写入共享退避参数；不可热回滚 + 待重启清单 | `hot_change_applies_without_restart`、`semi_change_is_reported_as_next_use`、`cold_change_is_rolled_back_and_listed_for_restart`、`crates/app/tests/config_state.rs::semi_change_updates_the_shared_backoff` |
| 4.14 | 重载失败保留 last-good（坏 TOML / 未知键 / 非法过滤器三种） | `invalid_reload_keeps_last_good`、`crates/app/tests/config_state.rs::invalid_log_filter_rejects_the_whole_reload` |
| 4.15 | watch 里的配置 == 生效中的配置（发布走唯一写入口） | `crates/app/tests/config_state.rs::{hot_change_applies_and_is_published_to_watchers,published_view_equals_effective_config}`、`cold_change_is_rolled_back_in_the_published_view` |
| 4.16 | 无变化的重载产生空报告 | `unchanged_file_produces_an_empty_report` |

**限制**：`log.filter` 的合法性由装配层的 `EnvFilter::try_new` 判定（config 只做归一化）——
错误路径有测试（4.14 第三条），但"非法过滤器在启动时退出码 2"没有独立测试（启动路径需要真实二进制，
由 `make probe` 之外的场景覆盖；`lifecycle.rs::startup_failure_is_reported_before_any_task_runs`
覆盖的是另一个启动失败面）。

## 5. I5 生成契约本身成立

| # | 证据 | 位置 |
| --- | --- | --- |
| 5.1 | 唯一提示 `crate_prefix`；`project-name` / `crate_name` 独立 | `cargo-generate.toml`（`[placeholders.crate_prefix]` 仅一项）；`hooks/pre.rhai` 校验两者形状 |
| 5.2 | 前缀形状被权威校验（合法/非法各 4/5 组夹具，错误消息指向 `crate_prefix`） | `make matrix`（`scripts/prefix-fixtures.tsv`） |
| 5.3 | 极端长名 + 极端短名各一次真实换名生成，完整 `make check` 绿、不复用编译缓存、目录在仓库树外 | `make check-gen`（`scripts/names.tsv`） |
| 5.4 | 上限长度前缀（32 字符）下 `cargo fmt --check` 仍然干净 | `make check-gen` 的 long 行（前缀 `abcdefghijklmnopqrstuvwxyz012345`） |
| 5.5 | `make check` 是门禁唯一定义（fmt-check / lint / test），不需要非 Rust 工具链 | 生成结果 `Makefile`；`make check-gen` 实际执行 |
| 5.6 | 切片一（加任务面）：照 README 走通，`make check` 绿 + 进程内出现 `stopped task=flush runtime=main` | `make slices`（task） |
| 5.7 | 切片二（加仓储）：迁移 + 仓储 + 接线走通，`make check` 绿 + `notes written` + `stopped task=notes-writer` | `make slices`（repo） |
| 5.8 | README 代码块与 `scripts/slices/` 答案文件逐字一致（反漂移） | `make slices-check`（`tools/audit slices`） |
| 5.9 | 生成结果里没有模板材料 / 里程碑词 / 阶段编号 / 模板门禁目标名 | `make check-gen` 内的生成结果审计（61 个文件逐个扫描） |
| 5.10 | 生成结果不带 CI 配置 | `tools/audit` 生成结果审计（没有 `.github` 等目录；`REQUIRED_IN_GENERATED` 与目录扫描） |
| 5.11 | 模板树干净（顶层白名单、无垃圾、无符号链接、ignore 条目存在） | `make audit` |
| 5.12 | 骨架不预建表（迁移目录为空） | `make audit`（迁移目录检查） |
| 5.13 | 生成契约的版本区间被声明与实测 | `cargo-generate.toml` 的 `cargo_generate_version = ">=0.24.0, <0.25.0"`；`make gate` 全部在 0.24.0 上跑过 |

**限制**：极端长名的长度是 58 字符（`extremely-long-project-name-used-for-template-verification`），
不是无限长；更长的 `--name` 只会影响 README 标题、二进制名与 `[[bin]]`（都不经过 rustfmt），
所以没有自动化证据专门覆盖"更长的名字"。

## 6. 支撑约束

| # | 证据 | 位置 |
| --- | --- | --- |
| 6.1 | 零业务全局态：源码扫描（禁 `OnceLock` / `LazyLock` / `lazy_static` / `once_cell` / `static mut` / `static` 可变容器） | `crates/app/tests/structure.rs::no_global_state_in_crates` |
| 6.2 | 一个进程里两套独立装配并发（生命周期、配置、数据库、取消互不干扰） | `crates/app/tests/isolation.rs::two_assemblies_run_concurrently_without_interference` |
| 6.3 | 可观测性只在装配层初始化：库 crate 里不出现 subscriber 相关符号 | `crates/app/tests/structure.rs::no_subscriber_init_in_libs` |
| 6.4 | 日志形态只有一种（单行、带 target、无 ANSI 颜色），没有格式开关 | `crates/app/src/telemetry.rs`（唯一 subscriber，硬编码 `fmt::layer`） |
| 6.5 | 日志里不出现 secret：`Secret` 不实现 `Display`、`Debug` 渲染 `***` | `crates/config/src/secret.rs::tests::debug_never_renders_the_value` |
| 6.6 | 测试不装全局 subscriber：所有断言走 `ShutdownReport.records` 与结构化记录 | 全部 `crates/runtime/tests/*`、`crates/app/tests/*` |
| 6.7 | 每条第三方依赖旁有"为什么" | `crates/app/tests/structure.rs::workspace_deps_have_reasons_and_are_used`；`docs/references.md §1` |

**限制**：6.1 的扫描是文本级（可能漏掉跨行的可变 static 声明）；6.4 是"代码里只有一处 subscriber"
的构造性事实加 `make probe` 的日志观察，不是运行时断言。

## 7. DoD 对照

| DoD | 证据 |
| --- | --- |
| 1 模板门禁全绿且覆盖邻接表、渲染面/泄漏审计、生成结果 `make check` | `make gate` = audit + slices-check + template-fmt + matrix + check-gen + probe + slices |
| 2 真实换名生成（cargo-generate 展开路径、不复用缓存、仓库树外；长/短名） | `make check-gen` |
| 3 干净目录：`make check` 绿；`cargo run` 首条日志有构建串；Ctrl-C 后每个任务 `stopped`；退出码正确 | `make probe`（真实 SIGINT）+ `crates/app/tests/lifecycle.rs::clean_stop_reports_every_task_stopped`（进程内版本） |
| 4 两条 README 切片实际走通 | `make slices` |
| 5 生成结果无模板材料/里程碑/门禁目标名 | `make check-gen` 内的生成结果审计 |
| 6 每条不变量有证据或显式"无自动化证据" | 本文 |
| 7 每条依赖有理由 | 6.7 |
| 8 零配置可起、锚点正确、换目录读同一份配置 | 4.2/4.3/4.4/4.5 |
| 9 每段关停预算有上限且被测试钉住；收不回的如实上报 | 3.1–3.4 |
