# 模板仓库自己的门禁。
#
# 注意这**不是**生成结果的 Makefile——那份是 `Makefile.project`，它在生成时被改名过去，
# 只有 `fmt` / `lint` / `test` 三条。两份文件验的是两件不同的事：
#
#   这一份    ——「模板展开出来的东西对不对」。跑的是 shell，读的是生成树。
#   project   ——「这个工程本身对不对」。跑的是 cargo，读的是它自己的源码。
#
# 把两者合成一份是做不到的：模板根的 `Cargo.toml` 里写着 `{{crate_prefix}}-core`，
# cargo 连清单都读不完，所以这一份里没有任何一条能在模板根上直接跑的 cargo 命令。
#
# ## 唯一入口
#
# `make check` 是模板侧全部门禁的并集。加检查的方式是挂到它的依赖链上，不是新开一个
# 需要人记得单独跑的目标——一个需要人记得的门禁，可以在半年里一次都没执行过而 CI 全绿。
#
# 例外只有两个，都是**故意**留在 `check` 外面的，理由都是同一个：它们慢到会让人开始
# 躲着 `check` 走。
#
#   `make verify`      —— 走 cargo-generate 的 `--test` 展开路径，**不复用编译缓存**，
#                         从零编一遍生成结果（§11.4）。
#   `make verify-git`  —— 走 `--git` 安装路径，验 `.gitattributes` 的换行符中立性，
#                         并在克隆出来的树上再跑一遍结构检查。
#
# 这两条在 CI 里各占一格，各跑一次。把它们排除在 `check` 之外是一个有代价的选择：
# 本地只跑 `check` 的人不会碰到它们能逮住的问题。代价由 CI 接着——这是它们必须
# **在 CI 里**各有一格的理由，而不是"有空再跑"。
#
# ## 第五个目标：`lock`
#
# `make lock` 是**唯一一个往模板仓库里写文件的目标**。它不在 `check` 里，也永远不该进去：
# 一个会顺手改工作树的检查，会让"门禁绿了"和"我的改动被改过了"混在一起。
#
# ## 并行
#
# `.NOTPARALLEL` 是必须的。全部子目标共享 `$(GATE_ROOT)`，`scripts/lib.sh::gate_lock`
# 会把它们串起来——于是 `make -j8` 的结果不是快八倍，是七个进程排队等锁，等满 600 秒然后
# 红在一个与被测对象毫无关系的地方。声明出来，比让人自己撞上去好。

SHELL := /bin/bash

# 门禁工作区。默认由 `scripts/lib.sh` 从 `$TMPDIR` 算出来；这里问它要，而不是在两个
# 地方各算一遍——`clean` 要删的必须**正好是**门禁写过的那个目录。
GATE_ROOT ?= $(shell bash -c '. scripts/lib.sh >/dev/null 2>&1 && printf %s "$$GATE_ROOT"')

.PHONY: help check verify verify-git lock clean gen \
        preflight tooling-test acceptance-ids structure project-check matrix \
        migration-rebuild release-probe

.DEFAULT_GOAL := help
.NOTPARALLEL:

help:
	@echo '模板仓库的门禁。生成结果自己的门禁是另一份（Makefile.project → 生成后的 Makefile）。'
	@echo ''
	@echo 'make check      —— 模板侧全部门禁。改完模板跑这一条。'
	@echo 'make verify     —— 冷编译 + cargo-generate --test 展开路径。慢，CI 单跑一格。'
	@echo 'make verify-git —— --git 安装路径 + 换行符中立性。CI 单跑一格。'
	@echo 'make lock       —— 刷新模板的 Cargo.lock。唯一会改工作树的目标。'
	@echo 'make clean      —— 删掉门禁工作区 $(GATE_ROOT)。'
	@echo ''
	@echo '单独跑某一条：make preflight / tooling-test / acceptance-ids / gen / structure /'
	@echo '              project-check / matrix / migration-rebuild / release-probe'

# ── 唯一入口 ────────────────────────────────────────────────────────────────
#
# 顺序是**由便宜到贵**，不是按重要性：preflight 秒级，tooling-test 不碰 cargo，
# gen/structure 只读文件，到 project-check 才第一次编译。环境坏了的时候，红在第一条
# 而不是三分钟以后。
check: preflight tooling-test acceptance-ids gen structure project-check matrix migration-rebuild release-probe
	@echo ''
	@echo '✓ check 通过：模板侧九条门禁全绿。'
	@echo '  注意 verify 与 verify-git 不在这条里面（见 Makefile 头注释），CI 各跑一次。'

# ── 子目标 ──────────────────────────────────────────────────────────────────
#
# 每一个依赖外部工具的目标都声明 `preflight`（B-28）。重复声明不是冗余：make 一次调用里
# 只会跑它一遍，而少写的那一个恰恰是单独调用时会给出烂诊断的那个。
preflight:
	@bash scripts/preflight.sh

tooling-test: preflight
	@bash scripts/tooling-test.sh

# 纯文本比对，不碰 cargo、不生成树，所以排在 gen 前面：它是 `check` 里最便宜的几条之一，
# 而验收清单失配是改完代码最容易忘的一件事——让它早点红。
acceptance-ids: preflight
	@bash scripts/acceptance-ids.sh

gen: preflight
	@bash scripts/gen.sh

# 这两条读的是 `gen` 留下的**同一棵**树。各自再生成一次的话，structure 验过的树和
# project-check 跑 `make check` 的树就成了两棵，而它们之间没有任何东西保证一致。
structure: preflight gen
	@bash scripts/structure.sh

project-check: preflight gen
	@bash scripts/project-check.sh

matrix: preflight
	@bash scripts/matrix.sh

migration-rebuild: preflight
	@bash scripts/migration-rebuild.sh

release-probe: preflight
	@bash scripts/release-probe.sh

# ── check 之外的两条独立入口 ────────────────────────────────────────────────
#
# `env -u CARGO_TARGET_DIR`：这一趟的价值之一就是从零编译，而不少人会在 shell 里全局
# 设一个共用的 target 目录。脚本自己也拦这件事（它会红，并告诉你怎么改）——那层拦截保的是
# 直接调脚本的路径。这里选择**替用户剥掉**而不是让他红：`make verify` 是受支持的入口，
# 一个因为用户有个完全正当的环境变量就失败的入口，只会教人跳过它。
verify: preflight
	@env -u CARGO_TARGET_DIR bash scripts/verify.sh

verify-git: preflight
	@bash scripts/verify-git.sh

# ── 唯一的写入口 ────────────────────────────────────────────────────────────
lock: preflight
	@bash scripts/normalize-lock.sh

# ── 清理 ────────────────────────────────────────────────────────────────────
#
# 借 `safe_rm_rf` 的四层护栏，而不是在这里写一条裸的 `rm -rf $(GATE_ROOT)`：那个变量
# 万一算成了空串，`rm -rf` 会把 `$(CURDIR)` 底下的东西当成目标。护栏里第 2 层专门拦这个。
clean:
	@bash -c '. scripts/lib.sh; gate_workspace_init; safe_rm_rf "$$GATE_ROOT"; \
	          printf "已删除门禁工作区：%s\n" "$$GATE_ROOT"'
