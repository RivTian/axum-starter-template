# 验收清单

这份文件列出模板**声称具备**的每一项能力，以及证明它的用例名。

## 验收 ID 就是用例名

没有另一套编号。`A-17` 这样的编号在源码里不存在，所以它可以在文档里活得好好的而对应的
用例早就改名或删掉了——上一版就是这么烂的：55 个验收 ID 拿去 `.rs` 里搜，**零命中**。
再发一套编号只会把这件事重演一遍。

用函数名当 ID 换来一条硬性质：**改名就是改 ID**。改完不同步这份文件，`make check` 当场红。

代价是名字很长，而且每加一条用例就要在这里加一行。这正是要的那个代价——它是这条纪律
唯一的执行力来源。

## 为什么没有"重新生成这份文件"的命令

因为那个命令会让这道门禁变成零。

一条"文档与源码必须相等"的检查，只有在**同步是手工的**时候才有意义：手工同步逼着人在
加一行之前看一眼这条用例属于哪项能力，也逼着人在删一行时想一下那项能力是不是没了。
给它配一个 `make regen-acceptance`，所有人都会先跑那条命令再看红绿，于是这份文件永远
等于源码，而"永远相等"和"没有检查"是同一件事。

这份清单**只被机械地生成过一次**（建档时），之后全部手工维护。

## 门禁怎么读它

`scripts/acceptance-ids.sh`（`make check` 的一格）做两个方向的集合比对：

- 源码侧：`.rs` 里每个 `#[test]` / `#[tokio::test]` 紧跟的函数名。
- 文档侧：本文件里每一行**行首**形如 `` - `name` `` 的条目。

两侧集合必须**完全相等**。少了报"文档声称了一条不存在的用例"，多了报"有用例没被任何
一项能力认领"。后者同样是错误：一条不属于任何声称能力的用例，说明要么能力漏写了，要么
这条用例在验一件没人想验的事。

正文里提到用例名时用的是行内形态（"这条由 `a_clean_shutdown_succeeds` 证明"），不是行首
条目，所以说明文字不会被误当成清单项。

## 这里没有的东西

本文件回答"什么被证明了"。**没有**被自动化证明的部分——以及每一条的影响——在
`docs/verification.md`。两份文件合起来才覆盖全部纪律，单看这一份会高估覆盖面。

一条纪律要么出现在这里并带着用例名，要么出现在那里并带着「无自动化证据」的标注。
不允许留白，也不允许一条能力在两边都找不到。

点名一遍，免得"去那边看"变成没人去看。下面十四条**在本文件里一条用例都没有**，理由、
残余风险、以及本次有没有手工证据，逐条写在 `docs/verification.md` 第 4 节：

| # | 没有自动化证据的事实 | # | 没有自动化证据的事实 |
| --- | --- | --- | --- |
| 1 | `tokio::signal` 真实投递信号 | 8 | 进程级线程计数 |
| 2 | `current_exe()` 自身的返回值 | 9 | Windows 文件权限与一切非 unix 平台 |
| 3 | 退出码被操作系统观察到 | 10 | Linux |
| 4 | `StorageError::VersionConflict`（无构造点） | 11 | `.github/workflows/ci.yml` 整体 |
| 5 | 半热档"下次面重启生效"的跨重启证据 | 12 | `cargo install cargo-generate` 在 1.95.0 上编过 |
| 6 | symlink 农场下的安装根 | 13 | `cargo audit --file` 对占位符锁文件的行为 |
| 7 | `Secret` 四条出口用例之外的不回显 | 14 | 生成结果的 release 构建 |

这张表刻意用表格而不是列表：本文件里行首形如 `` - `name` `` 的条目会被 `acceptance-ids`
当成验收 ID 去源码里找同名用例，而这十四条按定义**没有**用例——写成条目会逼着人去补一条
假测试来让门禁变绿，正好是这套纪律要防的事。

---

## 1. 配置：三段式装配、冷热分级、密钥与路径（79）

配置的完整形态由 `core` 定义、由 `app` 装配。这一节覆盖：三个来源按声明优先级依次尝试、
`${VAR}` 展开、`validate → clamp → finalize` 的固定次序、冷/半热/热三级字段的分桶与
「冷字段变更整批拒绝」、`watch` 发布的世代号单调递增、以及密钥在任何一条路径上都不回显
原值（包括反序列化失败的错误消息——`the_unprotected_form_would_have_leaked_it` 是那条的
负向对照，它证明不加保护时**确实**会泄漏）。

路径锚定的规则是"相对路径锚到安装根，绝对路径原样保留"，且安装根取自可执行文件所在目录；
`a_different_anchor_moves_everything_together` 保证这套规则换个锚点时整体平移，不会有
哪一项偷偷留在旧位置。

- `relative_storage_paths_anchor_to_the_install_root`
- `absolute_storage_paths_are_left_alone`
- `resolve_paths_is_idempotent`
- `a_whitespace_only_filter_becomes_empty_then_invalid`
- `validate_rejects_only_what_cannot_possibly_run`
- `out_of_range_values_are_clamped_and_recorded`
- `default_config_declares_no_extra_runtime`
- `a_plane_bound_to_a_declared_runtime_passes`
- `the_three_runtime_mistakes_are_startup_errors`
- `a_runtime_name_never_reaches_the_message_when_it_is_a_value`
- `an_extra_runtime_gets_the_same_clamping_as_the_main_one`
- `a_clamp_record_path_cannot_forge_a_log_line`
- `clamping_is_idempotent_and_silent_when_nothing_is_out_of_range`
- `none_thread_counts_survive_clamping`
- `an_empty_storage_path_is_rejected_rather_than_becoming_the_install_root`
- `finalize_runs_the_stages_in_the_only_correct_order`
- `finalize_stops_at_validate_without_clamping`
- `identical_configs_produce_an_empty_diff`
- `every_cold_field_is_reported_on_its_own_path`
- `cold_field_list_is_complete`
- `semi_and_hot_fields_land_in_their_own_buckets`
- `cold_change_is_never_silently_accepted`
- `a_hot_only_change_is_accepted_whole`
- `a_mixed_change_is_rejected_whole_not_partially_applied`
- `the_watch_value_is_the_effective_config`
- `generation_advances_by_one_per_publish`
- `generations_are_ordered_so_stale_values_can_be_detected`
- `every_reader_sees_the_same_value`
- `publishing_with_no_readers_left_is_not_an_error`
- `a_snapshot_taken_before_a_publish_keeps_its_own_value`
- `supported_forms_expand`
- `unsupported_shell_forms_are_rejected_not_passed_through`
- `stray_dollar_tells_you_to_escape_it`
- `undefined_without_default_is_an_error`
- `unterminated_placeholder_reports_the_dollar_offset`
- `errors_never_echo_the_variable_value`
- `secret_debug_never_prints_the_value`
- `a_secret_nested_in_a_struct_is_still_redacted`
- `sources_are_tried_in_the_declared_priority`
- `a_declared_source_that_yields_nothing_is_an_error_not_a_fallback`
- `an_empty_source_block_is_an_error`
- `file_contents_lose_their_trailing_newline`
- `secret_deserialize_error_never_contains_the_value`
- `the_unprotected_form_would_have_leaked_it`
- `a_well_formed_secret_still_deserializes`
- `defaults_contain_no_deployment_facts`
- `default_budgets_leave_headroom_under_the_hard_bound`
- `an_empty_document_yields_the_defaults`
- `log_format_names_are_stable`
- `control_characters_are_stripped`
- `overlong_paths_are_truncated_with_a_visible_marker`
- `truncation_does_not_split_a_multibyte_character`
- `config_error_never_echoes_values`
- `install_root_is_the_executable_directory`
- `bare_file_name_falls_back_to_dot_instead_of_panicking`
- `config_and_data_are_siblings`
- `relative_paths_anchor_to_root_and_absolute_ones_do_not`
- `a_different_anchor_moves_everything_together`
- `embedded_template_round_trips_to_default`
- `the_default_location_is_created_on_first_start`
- `an_explicit_path_is_never_created`
- `the_three_sources_are_tried_in_order`
- `an_empty_environment_variable_is_a_path_not_an_absence`
- `the_env_prefix_comes_only_from_the_binary_name`
- `environment_variables_are_expanded_in_values`
- `a_syntax_error_reports_a_line_and_column`
- `a_type_error_reports_the_field_path`
- `an_oversized_file_fails_instead_of_being_truncated`
- `relative_paths_are_anchored_to_the_install_root`
- `an_unknown_key_is_rejected_rather_than_ignored`
- `a_non_utf8_variable_is_dropped_rather_than_mangled`
- `reload_with_missing_file_fails_and_keeps_last_good`
- `reload_never_recreates_a_deleted_config_file`
- `a_hot_change_is_applied_and_advances_the_generation`
- `a_cold_change_rejects_the_whole_batch`
- `an_identical_file_does_not_advance_the_generation`
- `a_malformed_file_keeps_last_good`
- `a_clamped_value_is_reported_with_the_reload`
- `debug_does_not_leak_environment_values`

## 2. 任务面与监督（29）

顶层任务面由 `TaskSupervisor` 统一持有。这一节覆盖：任务名唯一、结束原因四分
（正常返回 / 返回错误 / panic / 被取消）且**互不混淆**——`panic_is_classified_as_panicked_not_cancelled`
是其中最容易写错的一条，因为 tokio 把 panic 和 abort 都表达成 `JoinError`。

`abortable` 与 `graceful` 两类任务的预算归属由规格决定而不是由装配层现编
（`abortable_task_gets_no_harvest_budget`）。关停预算的分配保证内层永不超出外层
（`inner_stage_never_outlives_the_outer_deadline`），且升级只会收紧不会延长。

`next_exit_survives_being_dropped_inside_select` 钉的是取消安全：整个 future 被
`select!` 丢掉时不能吞掉已经取到的退出记录。

- `duplicate_task_name_is_a_runtime_error`
- `empty_supervisor_exits_normally`
- `returned_and_failed_are_distinguishable`
- `panic_is_classified_as_panicked_not_cancelled`
- `abortable_task_is_not_waited_for_in_harvest`
- `a_cooperative_shutdown_is_not_reported_as_forced`
- `drop_aborts_outstanding_tasks`
- `next_exit_on_empty_supervisor_is_none`
- `next_exit_reports_a_failing_plane_before_any_deadline`
- `next_exit_survives_being_dropped_inside_select`
- `a_sent_ack_arrives_with_the_name_the_assembler_chose`
- `a_dropped_sender_reads_as_a_missing_ack_not_as_a_hang`
- `sending_into_a_closed_channel_is_silent`
- `missing_name_does_not_erase_the_panic`
- `terminal_and_failure_are_orthogonal`
- `panic_summary_extracts_both_std_payload_shapes`
- `abortable_task_gets_no_harvest_budget`
- `spec_requires_every_field`
- `exhaustive_variant_list`
- `short_names_are_unique`
- `only_running_serves_traffic`
- `publish_without_subscribers_still_records_the_phase`
- `wait_for_running_returns_false_when_shutdown_overtakes_it`
- `wait_for_running_observes_a_later_transition`
- `stages_are_monotonically_ordered`
- `inner_stage_never_outlives_the_outer_deadline`
- `overflow_is_reported_not_swallowed`
- `escalation_tightens_and_never_extends`
- `reap_gives_up_at_its_own_deadline`

## 3. 关停编排（21）

信号到来之后的那条路径：第一次请求开始收尾并记录原因，第二次**收紧**预算但不放弃当前
阶段，第三次放弃阶段强制退出。三条里最容易写反的是"第二次不能延长"——
`a_very_late_second_request_still_never_extends` 专门钉住迟到很久的第二次请求。

关停报告的真实性由 `a_forced_exit_never_claims_a_clean_close` 和
`a_skipped_pool_close_is_not_a_success` 两条守住：报告里说"干净关停"必须六个合取项全部
成立，而 `each_of_the_six_conjuncts_can_fail_on_its_own` 逐个把它们打掉一次，证明这个
结论不是恒真的。

`a_clean_shutdown_announces_before_it_cancels_and_closes_storage_last` 是这一节唯一一条
走完整个 `orchestrate` 的用例，它同时钉三件事：公告先于第一条任务退出、每个在册的面都留下
闭合记录、存储在所有面退完之后才关。它喂进去的是一个**编排好的**信号流
（`Canned`），不是真的 SIGINT——真实信号的投递不在自动化证据里，见 `docs/verification.md`。

- `a_finished_stage_wins_over_a_pending_signal`
- `a_second_stop_request_tightens_the_plan_without_abandoning_the_stage`
- `a_third_stop_request_abandons_the_stage`
- `sighup_is_ignored_while_draining`
- `an_ended_signal_stream_reports_once_and_then_hangs`
- `an_ack_dropped_abort_carries_the_reason_recovered_from_the_harvest`
- `an_abort_without_a_recovered_exit_still_says_something_useful`
- `a_forced_exit_never_claims_a_clean_close`
- `the_first_request_begins_and_records_its_cause`
- `the_second_request_tightens_and_never_extends`
- `a_very_late_second_request_still_never_extends`
- `the_second_request_keeps_the_first_cause`
- `the_third_request_forces_an_exit_and_stays_forced`
- `only_a_signal_counts_as_an_expected_stop`
- `a_clean_shutdown_succeeds`
- `each_of_the_six_conjuncts_can_fail_on_its_own`
- `a_skipped_pool_close_is_not_a_success`
- `a_startup_failure_never_succeeds_and_keeps_the_source_chain`
- `a_shutdown_with_no_tasks_at_all_still_reports_its_cause`
- `forced_shutdown_reports_failure_not_success`
- `a_clean_shutdown_announces_before_it_cancels_and_closes_storage_last`

## 4. 存储门面与 SQLite 后端（30）

门面纪律：上层只见 `Arc<dyn Storage>`，看不见 sqlx 的任何类型。
`both_backends_satisfy_the_same_contract` 拿同一组断言跑两个后端，是这条纪律唯一真正的
证据——只有一个实现时，"抽象成立"是一句无法证伪的话。

SQLite 侧覆盖：文件与两个 sidecar 的属主位、读写池物理分离（`reader_pool_really_refuses_writes`
证明只读池**真的**拒绝写，不是靠约定）、pragma 取到的是我们要的值而不是 sqlite 的默认值、
迁移被改动时快速失败而不是"自动修复"。

错误映射把 sqlx 的错误收敛成门面错误种类，且 `internal_context_never_embeds_upstream_text`
保证上游文本不会顺着错误链爬到响应里。

- `error_messages_carry_no_payload_beyond_safe_fields`
- `only_a_confirmed_close_counts_as_clean`
- `timed_out_is_a_failure_with_a_stable_wording`
- `open_creates_the_file_and_the_directory_above_it`
- `the_database_and_both_sidecars_are_owner_only`
- `an_existing_loose_file_is_tightened_but_owner_bits_are_kept`
- `migration_set_creates_no_business_tables`
- `opening_twice_is_idempotent`
- `a_cancelled_open_says_which_step_it_stopped_at`
- `a_modified_migration_fails_fast_and_is_not_repaired`
- `a_failed_open_leaves_no_live_pool`
- `close_past_its_deadline_is_reported_as_failure_not_as_closed`
- `writer_pool_is_physically_single_writer`
- `pragmas_are_what_we_asked_for_not_what_sqlite_defaults_to`
- `reader_pool_really_refuses_writes`
- `debug_of_the_facade_leaks_nothing_but_the_backend_name`
- `health_fails_once_the_pool_is_closed`
- `no_rows_maps_to_not_found`
- `unique_violation_carries_the_constraint_name`
- `other_constraint_kinds_collapse_into_conflict`
- `every_migration_stage_is_reachable_and_carries_a_hint`
- `migrate_errors_nested_in_sqlx_error_are_not_swallowed`
- `internal_context_never_embeds_upstream_text`
- `faults_can_be_injected_and_taken_back`
- `debug_shows_the_backend_name_only`
- `open_creates_the_database_then_health_passes_then_close_is_clean`
- `the_facade_debug_says_nothing_but_the_backend`
- `the_facade_outlives_nothing_it_should_not`
- `both_backends_satisfy_the_same_contract`
- `injected_faults_come_back_as_the_facade_error_kinds`

## 5. HTTP 表面：错误信封与系统端点（28）

所有错误响应共用一种形状（三个键），且 `every_error_path_renders_the_same_three_key_envelope`
逐条走遍每一条错误路径而不是抽样。成功响应不复用那个键
（`success_bodies_never_use_an_envelope_key`）——两者混用会让客户端只能靠状态码分辨。

信息泄漏边界由三条守住：服务端原因丢细节、客户端原因保留细节
（`a_server_side_rejection_loses_its_detail_and_a_client_side_one_keeps_it`）、panic 载荷
只进日志不进 body、存储错误的 payload 不上响应。

超时回 **504** 而不是 408，理由是 RFC 9110 §15.5.9 把 408 定义成客户端的责任；
`the_timeout_is_a_gateway_timeout_and_says_so_in_its_code` 让"有人把它改回 408"当场变红。

最后一条走的是真实的 socket：`the_plane_acks_then_serves_then_returns_cleanly_on_cancel`
起一个真的监听器、发一个真的 HTTP 请求、再取消它，把「ack → 服务 → 干净返回 → 端口确实
释放了」四段连起来。它同时是 `from_std` 那步注册的唯一证据——fd 注册到了错的 runtime 上
时，listener 会「看起来正常、但永远不会就绪」，而所有走 `oneshot` 的用例照样全绿。

- `every_storage_variant_has_a_status_and_a_stable_code`
- `storage_statuses_split_client_faults_from_server_faults`
- `a_storage_payload_never_reaches_the_response_body`
- `a_storage_error_two_hops_down_still_decides_the_status`
- `an_error_chain_without_a_storage_error_is_a_plain_500`
- `a_server_side_rejection_loses_its_detail_and_a_client_side_one_keeps_it`
- `the_timeout_is_a_gateway_timeout_and_says_so_in_its_code`
- `fallback_body_is_a_well_formed_envelope`
- `a_rendered_envelope_echoes_its_own_status`
- `only_the_running_phase_is_serving`
- `the_readiness_reasons_never_collide_with_a_phase_name`
- `the_three_payload_shapes_are_all_described`
- `the_response_is_a_500_and_does_not_leak_the_payload`
- `the_shipped_business_router_adds_nothing`
- `the_three_system_endpoints_answer_three_different_questions`
- `readiness_names_each_way_of_not_being_ready`
- `a_probe_that_spans_a_phase_change_is_not_ready`
- `info_echoes_the_build_info_it_was_given`
- `success_bodies_never_use_an_envelope_key`
- `every_error_path_renders_the_same_three_key_envelope`
- `a_panic_payload_reaches_the_log_and_not_the_body`
- `a_handler_that_overruns_its_budget_is_a_504_envelope`
- `the_four_extractor_failures_keep_four_different_statuses`
- `a_query_rejection_is_an_envelope_too`
- `a_path_extractor_misuse_is_a_500_and_withholds_its_detail`
- `the_spec_says_graceful_and_the_plane_owns_that_answer`
- `a_taken_port_fails_at_bind_time`
- `the_plane_acks_then_serves_then_returns_cleanly_on_cancel`

## 6. 启动装配、运行期与进程边界（35）

装配的次序纪律：`launch_is_the_only_place_that_spawns`（派生只有一个落点）、
`commit_never_precedes_a_pending_ack`（没收齐 ack 不算启动成功）、启动期收到停止请求
要在别的事之前中止。四条失败路径各有一条用例，包括最容易写成挂死的那条——
`an_empty_supervisor_aborts_instead_of_hanging`。

运行期：默认配置只建一个 runtime 且**不多起线程**（`single_runtime_spawns_no_extra_threads`）；
声明了附加运行期时，关停按声明的反序进行、主运行期最后。

CLI 与遥测：`usage_contains_no_project_name_literal` 保证帮助文本里没有硬编码的项目名
（模板展开后名字会变）；`ansi_is_decided_by_the_injected_terminal_flag_only` 把 ANSI 开关
限制成一个注入的标志，因为 `with_ansi(true)` 在没开 `ansi` feature 时是**运行期 panic**。

两条端到端用例在这一层：`a_synthetic_stop_runs_the_whole_process_and_reports_a_clean_shutdown`
走完整条启动到关停的路径，`an_unreadable_config_fails_startup_and_never_forges_a_log_line`
是它的失败侧对照。

- `no_arguments_means_run_with_no_explicit_config`
- `config_accepts_both_spellings`
- `a_recognized_option_without_a_value_fails_fast`
- `the_next_option_is_not_swallowed_as_a_value`
- `unknown_options_and_positionals_are_rejected`
- `help_and_version_are_recognized`
- `usage_contains_no_project_name_literal`
- `capture_reports_the_bin_name_it_was_given`
- `a_non_utf8_value_reads_as_absent`
- `bind_failure_aborts_boot_with_the_original_error`
- `an_unknown_runtime_fails_assembly_instead_of_falling_back_to_main`
- `a_disabled_worker_is_never_registered`
- `launch_is_the_only_place_that_spawns`
- `commit_never_precedes_a_pending_ack`
- `a_plane_that_dies_before_acking_aborts_the_boot_with_its_reason`
- `a_dropped_ack_aborts_instead_of_waiting_for_a_reason`
- `a_stop_request_during_boot_aborts_before_anything_else`
- `an_empty_supervisor_aborts_instead_of_hanging`
- `a_default_config_creates_exactly_one_runtime`
- `single_runtime_shutdown_visits_exactly_one_runtime`
- `resolve_happens_once_at_startup`
- `single_runtime_spawns_no_extra_threads`
- `extras_shut_down_in_reverse_order_with_main_last`
- `executors_resolve_none_and_main_to_the_same_handle`
- `a_configured_thread_count_wins_over_available_parallelism`
- `text_format_carries_level_target_fields_and_message`
- `json_format_puts_fields_at_the_top_level`
- `ansi_is_decided_by_the_injected_terminal_flag_only`
- `an_invalid_filter_falls_back_and_says_so`
- `reload_is_not_a_stop_request`
- `every_variant_has_a_distinct_name`
- `a_different_executable_moves_config_data_and_the_logged_root_together`
- `a_synthetic_stop_runs_the_whole_process_and_reports_a_clean_shutdown`
- `an_unreadable_config_fails_startup_and_never_forges_a_log_line`
- `assemble_then_drop_leaves_no_task`

## 7. 示例工作者（7）

模板只带一个节拍器示例，业务留空。它需要验两件事。

一是**规格归属**："这个面要不要优雅收尾"由写这个面的人回答，而不是由装配层现编。

二是它作为**样板**的那一份责任：新写的面会照着它抄，所以它的生命周期必须是对的，而不只是
能跑。`the_plane_acks_before_the_commit_and_does_nothing_until_running` 钉住「先发回执、
在 `Running` 之前不干活」这个次序——抄错了的面会在配置还没提交的时候就开始发数据；
`a_boot_that_never_publishes_running_still_lets_the_plane_return_cleanly` 与
`shutdown_while_draining_is_also_a_clean_return` 各堵一条会挂死的路径。

节拍本身有三条：`it_ticks_once_per_interval_and_never_before_the_first_one_elapses`（第一次
节拍不能提前）、`a_reloaded_interval_takes_effect_after_the_round_that_already_read_the_old_one`
（热改间隔在**下一轮**生效，而不是打断当前这一轮）、
`cancellation_wins_over_a_tick_that_is_due_at_the_same_instant`（同一瞬间取消与节拍同时到期
时，取消赢——这正是 `every_select_puts_cancellation_first` 那条源码扫描想保证的行为）。

- `the_spec_says_abortable_and_the_plane_owns_that_answer`
- `the_plane_acks_before_the_commit_and_does_nothing_until_running`
- `a_boot_that_never_publishes_running_still_lets_the_plane_return_cleanly`
- `shutdown_while_draining_is_also_a_clean_return`
- `it_ticks_once_per_interval_and_never_before_the_first_one_elapses`
- `a_reloaded_interval_takes_effect_after_the_round_that_already_read_the_old_one`
- `cancellation_wins_over_a_tick_that_is_due_at_the_same_instant`

## 8. 构建信息（3）

版本号只有一个来源（`version_has_a_single_source`），服务名由调用方给出而不是在库里写死。

- `version_has_a_single_source`
- `the_service_name_is_whatever_the_caller_passed`
- `build_info_is_stable_across_calls`

## 9. 测试夹具与源码纪律（32）

`testkit` 自己也要被验：夹具的锚定规则必须和生产代码是同一条
（`the_anchor_rule_is_the_production_one`），否则测试会在一个生产里不存在的布局上全绿。
同一类的还有 `the_storage_replica_is_shared_not_copied`：`Rig` 交出去的那个存储替身必须
和 `AppState` 里的是同一个对象，否则「用例把存储改成不可用」这件事根本传不到 handler，
而依赖它的那条 `storage_unavailable` 用例会退化成一个永远绿的空断言。

`testkit/tests/discipline.rs` 的 20 条是**对源码本身的扫描**，它们把一批"读代码读不出来"
的纪律变成可执行的断言：分层依赖边等于分层表、工作区只有一个可执行入口、没有任何一层读
当前工作目录、没有任何一层取环境里的 runtime handle、每个 `select!` 都把取消放在第一个
分支、`allow` 属性挂在最小单元上、错误转换只跨工作区边界。

其中 `the_source_tree_keeps_the_shape_every_other_scan_assumes` 是这一组的前提条件：
其余扫描都假设源码树是某个形状，这条先把那个形状本身钉住——否则某次重构可以让另外 19 条
静默地扫了个空。

- `the_anchor_rule_is_the_production_one`
- `layout_is_created_and_matches_core_paths`
- `a_different_fixture_is_a_different_anchor`
- `config_can_be_written_at_the_default_name_and_at_another_one`
- `temp_db_hands_out_a_path_that_does_not_exist_yet`
- `everything_is_removed_on_drop`
- `named_events_are_captured_with_their_fields`
- `events_from_other_threads_are_captured`
- `order_is_recorded_so_sequencing_can_be_asserted`
- `capture_ends_at_drop`
- `install_is_idempotent`
- `the_source_tree_keeps_the_shape_every_other_scan_assumes`
- `every_public_export_is_listed_in_the_crate_readme`
- `the_dependency_edges_equal_the_layering_table`
- `the_test_fixtures_never_become_a_normal_dependency`
- `member_manifests_never_decide_a_version_themselves`
- `every_member_inherits_the_workspace_lints`
- `the_workspace_has_exactly_one_executable_entry_point`
- `task_names_are_never_compared_as_string_literals`
- `nothing_in_the_workspace_reads_the_current_working_directory`
- `no_layer_reaches_for_the_ambient_runtime_handle`
- `every_select_puts_cancellation_first`
- `allow_attributes_are_attached_to_the_smallest_unit`
- `error_conversions_only_cross_workspace_boundaries`
- `newtypes_with_smart_constructors_do_not_derive_deserialize`
- `the_process_boundary_adapters_contain_no_decisions`
- `main_carries_no_logic_of_its_own`
- `every_event_has_both_a_stable_name_and_a_human_message`
- `the_business_router_never_installs_a_fallback`
- `config_reload_does_not_grow_its_own_file_reader`
- `the_readme_names_make_check_as_the_only_gate`
- `the_storage_replica_is_shared_not_copied`
