# 完整模板验收证据索引

本页按架构 §12 的 55 个 ID 列出自动化证据与限制，不用测试总数替代逐项对应。
执行命令、最终报告、工具版本和平台范围见 [M5 验收记录](m5-verification.md)。
表中证据是组件测试、装配测试和真实进程测试的组合；不是对所有故障交错、任意业务扩展或所有操作系统的穷尽证明。

## 生命周期与 runtime

| ID | 可重复证据（仓库相对路径 / 测试名或场景） | 结论 / 限制 |
| --- | --- | --- |
| LIFE-01 | `api/src/contract_tests.rs::listener_binding_does_not_open_the_startup_commit_gate`；`worker/src/lib.rs::startup_gate_delays_work_and_cancellation_closes_the_task` | 监听/注册不等于 Running，门前不处理业务 |
| LIFE-02 | 上述真实组件启动门；`app/src/boot/tests.rs::missing_confirmation_without_task_exit_remains_bounded_by_startup_deadline` | 依赖显式确认，不靠任务注册顺序；缺确认有上限 |
| LIFE-03 | `boot/tests.rs::bind_failure_retains_the_task_error_instead_of_the_ack_channel_error`；storage migration tests；process `bind-failure-and-rollback` | 启动失败无提交，保留首因与资源清理 |
| LIFE-04 | process `signal-during-storage-initialization`；`a_stop_ready_before_boot_cancels_before_creating_data_resources`；worker 启动门取消 | 启动停止可观察；不是任意文件系统/驱动调用都能强制中断 |
| LIFE-05 | `app/src/supervisor/tests.rs::actual_results_keep_names_for_return_error_and_panic`；重复注册测试的外部 abort；`task_failure_wins_over_an_already_queued_stop` | 四种实际退出分类与统一失败关停路径的组合证据 |
| LIFE-06 | `boot/tests.rs::task_failure_wins_over_an_already_queued_stop` | 已就绪的 task failure 不被普通 stop 覆盖 |
| LIFE-07 | `app/tests/runtime_semantics.rs::children_cannot_cancel_their_parent_or_siblings`；process `sigterm-drain` / `runtime-shutdown-order` | 根级联、兄弟隔离、join/关池/关 runtime 顺序 |
| LIFE-08 | `runtime_semantics.rs::cooperative_async_abort_can_be_reaped_under_a_second_deadline`；`forcing_http_skips_normal_pool_close_even_after_the_outer_join` | 能被调度的 pending future 可 abort/reap，不假报 graceful |
| LIFE-09 | `boot/tests.rs::noncooperative_task_is_reported_unreaped_after_both_bounded_waits`；reload `started_blocking_work_is_not_falsely_reported_joined_when_aborted` | 两段等待都有限，测试最后释放闸门；不声称强杀 OS 线程 |
| LIFE-10 | `supervisor/tests.rs::duplicate_registration_and_closed_registration_never_spawn_the_future`；`runtime_semantics.rs::an_empty_joinset_returns_none_without_waiting` | 无副作用拒绝重名/停止后注册，空集不忙等 |
| LIFE-11 | `boot/tests.rs::second_stop_escalates_without_refreshing_the_timeline` | 第二次 stop 加速 Forcing，不刷新截止时间 |
| LIFE-12 | `boot/tests.rs::close_timeout_does_not_replace_the_original_failure` | 关池独立上限，首因保留，最终失败 |
| LIFE-13 | `api/src/tests.rs` 的 graceful/outer abort 两个真实连接测试；app `forcing_http_skips_normal_pool_close_even_after_the_outer_join` | 正常排空证明与外层 join 明确区分 |
| LIFE-14 | `boot/tests.rs::two_assemblies_in_one_process_have_independent_state_and_shutdown`；process `two-independent-processes` | 生命周期、配置、数据库与取消按实例隔离 |
| LIFE-15 | `boot/tests.rs::coordinator_unwind_cancels_real_tasks_and_preserves_the_external_deadline` | 协调器 unwind 取消自有任务，同步 runtime owner 保留原截止时间 |
| RT-01 | `app/src/rt/tests.rs::default_topology_allocates_no_extra_executor_collection_or_forwarders`；process `main` | 无 extra owner/forwarder 分配，不宣传逐字节零成本 |
| RT-02 | `rt/tests.rs::one_worker_extras_drive_tasks_without_an_extra_block_on_and_keep_named_exits`；process `worker`；`a_saturated_extra_worker_does_not_stall_main_http_or_its_shared_storage_probe` | 单 worker extra 被驱动、实际位置准确、主 runtime 可继续工作 |
| RT-03 | process `http/shared/split` 的 placement、ready、shutdown order | HTTP 在目标 runtime 绑定，共享池可跨 runtime 访问 |
| RT-04 | config runtime 名称/引用/预算测试；process `topology-fail-fast-before-construction` | 非法拓扑在线程/资源构建前拒绝 |
| RT-05 | `rt/tests.rs::partial_build_failure_closes_constructed_runtimes_and_preserves_the_original_error` | 第 k 次失败后逆序关闭已建 runtime，保留创建错误 |
| RT-06 | `rt/tests.rs::all_runtime_shutdowns_share_one_remaining_budget_not_n_full_timeouts`；`a_repeated_shutdown_cannot_refresh_the_original_cutoff` | 共享剩余等待预算，不线性叠加完整预算 |
| RT-07 | `app/tests/check_release.py` + `app/examples/m1_release_panic.rs` | 实际 release/unwind 产物中的 task panic、兄弟收割、runtime teardown 与退出 1 |

## 配置与 ticker

| ID | 可重复证据 | 结论 / 限制 |
| --- | --- | --- |
| CFG-01 | `app/src/config/tests.rs` 的 missing/oversized/non-UTF8、invalid fields；process `invalid-config-fail-fast` | 缺文件/非法值无默认掩盖，安全摘要错误 |
| CFG-02 | config `configuration_location_precedence_uses_only_explicit_inputs`、`paths_use_the_captured_configuration_directory_not_the_process_cwd`；process CLI/env 定位 | CLI > env > 默认；固定 path/cwd/CPU/env 快照，数据相对配置目录 |
| CFG-03 | config `env_strings_cannot_inject_toml_or_recursively_expand`、`non_utf8_environment_values_are_explicit_errors`、`errors_do_not_echo_configuration_values` | TOML 字符串内单次展开；不注入、不递归、不回显秘密 |
| CFG-04 | config `shipped_sample_and_omitted_defaults_are_equivalent`、范围/溢出 tests；shutdown `unrepresentable_budgets_fail_closed` | 示例与省略字段默认值一致，算术与范围失败关闭 |
| CFG-05 | `app/src/config/reload/tests.rs` 的 typed cold equality / hot changes；process reload 场景 | 冷热混改整批拒绝，仅有效热变化增代 |
| CFG-06 | `core/src/config.rs` 的 no change、无 receiver/late subscriber tests | 代号与值在同一 Arc，迟到订阅仍观察最新值 |
| CFG-07 | core `a_slow_reader_can_skip_generations_but_never_mixes_a_value_and_generation` | 允许跳代，借用不跨 await |
| CFG-08 | reload `timed_out_job_keeps_its_slot_and_a_burst_creates_only_one_followup`；boot `a_slow_reload_keeps_http_and_stop_responsive_and_is_reported_unreaped` | 单飞 + pending 位；过期仍占槽，HTTP/stop 不被 loader future 卡住 |
| CFG-09 | reload `shutdown_suppresses_a_late_result_and_never_starts_the_pending_job`；boot queued stop / grace completion tests；process `reload-shutdown-race` | 停止后不发布，不因晚结果恢复 Running |
| CFG-10 | core closed writer；worker `writer_closure_is_failure_unless_root_cancellation_already_won` | 意外关闭失败，正常取消优先，不忙循环 |
| TICK-01 | worker first full period/Skip、`changing_period_discards_the_old_deadline_without_an_extra_tick`；process `reload-second-period` | 提交后完整首周期、新周期连续两拍，Skip 不追补全部遗漏 tick |
| TICK-02 | worker writer closure/cancellation precedence 与周期重建测试 | 取消先于更新/tick；代号日志区分发布与消费 |

## 存储与 HTTP

| ID | 可重复证据 | 结论 / 限制 |
| --- | --- | --- |
| DB-01 | `storage/src/tests.rs::empty_migrations_bootstrap_only_sqlx_metadata_and_can_restart`；默认进程检查 | 空业务迁移仅创建必要元数据，仍连接/迁移/自检 |
| DB-02 | `storage/src/migration_tests.rs::a_real_migration_is_applied_once_and_validated_on_restart`；`scripts/probe_migrations.py` applied 两次 | fixture 版本只应用一次；真实嵌入服务也重跑验证 |
| DB-03 | migration tests `checksum_missing_version_dirty_and_sql_errors_fail_without_repair`；process migration-fail-fast；真实二进制增改删探针 | mismatch/missing/dirty/SQL 错误 fail-fast，不修元数据、不发布 facade |
| DB-04 | migration tests `an_exclusive_database_lock_times_out_without_publishing_storage`；storage lazy pool/closed facade；process storage-path/bind rollback | 打开/独占锁等待失败有界，所有者保留；关闭后的 health 失败。权限组合以实际 OS 用户/文件系统为准 |
| DB-05 | `scripts/template.py::migration_rebuild`；`.gitattributes` SQL LF 门禁 | 每次变更前先建立 fresh 基线；增改删后的真实二进制看到正确集合。跨平台 checkout 仍需对应 CI 运行证据 |
| DB-06 | migration tests `two_initializers_compete_boundedly_and_restart_revalidates_the_winner`、`an_interrupted_transactional_migration_is_revalidated_on_restart` | SQLite advisory lock 无排他保证；允许竞争者失败。中断证据是 SQLite progress handler 中断事务 fixture，不是断电/任意迁移完全回滚证明 |
| DB-07 | metadata 正常/dev/build 边与 feature/tree 检查；storage 私有 SQLx 类型/公开 health facade 的源码审查 | 编译闭包只有 SQLite；不把 Cargo.lock 的可选解析包误判为已编译后端 |
| HTTP-01 | `api/src/contract_tests.rs::health_and_info_do_not_query_storage_and_ready_requires_a_live_running_writer`；process system endpoints | 三端点数据形状和 DB 访问边界固定 |
| HTTP-02 | API storage failure/probe timeout、`a_probe_that_crosses_draining_does_not_return_ready` | 挂起/失败有界 503，最终生命周期检查阻止假就绪 |
| HTTP-03 | API root/nested/405/HEAD test；process `system-endpoints-and-errors` | JSON 404/405、Allow、HEAD 保持 |
| HTTP-04 | API `extractor_errors_keep_framework_statuses_without_leaking_values` | 错误媒体/JSON/形状/体积/Path/Query 状态保留，输入不回显 |
| HTTP-05 | API `the_outer_request_deadline_uses_the_same_json_error_envelope`；process `trace-redaction` | JSON 503，日志没有 query/header/body 秘密 |
| HTTP-06 | process `slow-header-shutdown-is-bounded-and-honest`、`disconnected-client-does-not-kill-the-face`；API held request/graceful/abort tests | 实际进程有外层退出 watchdog；正常或强制必须给出一致的清理证据，不要求残缺协议返回 JSON |
| HTTP-07 | API `a_request_panic_closes_its_connection_not_the_top_level_http_task` | 仅测试 panic 路由的连接结束后，HTTP 仍能处理下一请求；不假定 supervisor 收到 handler panic |

## 生成工程

| ID | 可重复证据 | 结论 / 限制 |
| --- | --- | --- |
| GEN-01 | `make matrix` 的 short/long/hyphen/registry-collision；`check_service.py --identity-only` | 包名/固定别名/二进制名/环境前缀均实际消费；最大长度真实编译 |
| GEN-02 | `structure` 的渲染白名单、Rust/CI 原样字节检查；`SyncSafetyTests` 的字面花括号/环境/GitHub 表达式用例 | 无反向全局替换，合法语法与 Liquid 输入分开 |
| GEN-03 | `structure` metadata/path/normal/dev/build/registry SQLx 检查 | 五 crate 单向边，无外部 path 依赖/第二后端/subscriber 越层；撞词不误报 |
| GEN-04 | 完整 gate 直接运行生成项目 `make check`；CI/Makefile byte equality；actionlint 静态校验 | 项目不依赖模板脚本，CI 分离。远端 runner 尚未触发，静态校验不是远端通过 |
| GEN-05 | `make check` + `make verify` | 四种名字真实 fmt/clippy/test/build，第二身份空 target 完整 gate；依赖下载缓存可复用，不称离线/全空 Cargo home |
| GEN-06 | `scripts/test_template.py` 目录/同步/命令 watchdog tests；matrix 实际并行同名生成、兄弟 clean、父 workspace 不变、直接 generator 非法名 | `--no-workspace` 防父 manifest 污染；fmt/lock 串行 + 快照拒绝可观察的编辑冲突；不支持把编辑器写入与格式回写视为原子事务 |
| GEN-07 | 生成项目 release-check、release 五布局进程与日志检查 | 不是使用 test profile 代替 release；同一进程检查器作用于实际 release artifact |

## 证据范围

- 已验证环境及最终运行状态以 M5 报告为准；当前不把 Linux/Windows 或远端 CI 列为已验收平台。
- 不包含任意业务 SQL 的迁移安全、网络文件系统卡死、不可中断 CPU/FFI 工作、断电恢复或生产负载 SLA。
- 新增真实业务后，应替换空迁移/ticker 的示例断言，保留本页的所有权、失败可观测与有界清理原则。
