# 模板仓库的 Makefile。
#
# 仓库本身**不可直接构建**：源码里是 `{{crate_prefix}}` 等 cargo-generate 占位符。
# 所有 cargo 命令都在「生成目录」里跑：`make gen` 用固定的开发用名字把模板展开到
# GEN_DIR，之后的门禁、运行、格式化都进那个目录。
#
# 生成目录放在仓库树**外**：cargo-generate 对 `--path` 模板是先整目录复制再套
# `ignore`（0.24 起会跳过 .git 与带 CACHEDIR.TAG 的目录，如 cargo 的 target/），
# 放树外是为了让 git status、rglob 同步脚本都不用绕着一个不属于模板的目录走。

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
CARGO ?= cargo

# 开发用的两个名字。带连字符的项目名让 project-name / crate_name / env_prefix
# 三个派生值两两不同，scripts/template-sync.py 的反向替换才不会撞车
GEN_ROOT ?= $(HOME)/.cache/axum-starter-template
GEN_NAME ?= tpl-project
GEN_PREFIX ?= tplx
GEN_DIR := $(GEN_ROOT)/$(GEN_NAME)
GEN_BIN := $(GEN_PREFIX)-app

GENERATE := $(CARGO) generate --path $(ROOT) --init --name $(GEN_NAME) \
	--define crate_prefix=$(GEN_PREFIX) --overwrite --silent

.DEFAULT_GOAL := help

.PHONY: help gen build release run test fmt fmt-check fmt-portable fmt-matrix lint coverage check verify lock clean

help:
	@echo "axum-starter-template — targets (all cargo work happens in $(GEN_DIR))"
	@echo
	@echo "  gen         expand the template into GEN_DIR (name=$(GEN_NAME), prefix=$(GEN_PREFIX))"
	@echo "  build       debug-build the generated workspace"
	@echo "  release     release-build the generated binary"
	@echo "  run         run the generated binary (debug)"
	@echo "  test        cargo test --workspace --locked"
	@echo "  fmt         cargo fmt in GEN_DIR, then sync formatted sources back into the template"
	@echo "  fmt-check   cargo fmt --check"
	@echo "  fmt-portable static scan: no line whose rustfmt layout depends on the chosen names"
	@echo "  fmt-matrix  re-run fmt-check under several other name pairs (FMT_MATRIX)"
	@echo "  lint        clippy --all-targets -D warnings"
	@echo "  coverage    cargo llvm-cov -> coverage/lcov.info"
	@echo "  check       fmt-portable + fmt-check + fmt-matrix + lint + test (the gate)"
	@echo "  verify      cargo generate --test: expand under a second set of names, build+test from scratch"
	@echo "  lock        regenerate Cargo.lock in GEN_DIR and copy it back with placeholders"
	@echo "  clean       remove GEN_DIR"
	@echo
	@echo "  GEN_ROOT / GEN_NAME / GEN_PREFIX / VERIFY_NAME / VERIFY_PREFIX can be overridden."

gen:
	mkdir -p $(GEN_DIR)
	cd $(GEN_DIR) && $(GENERATE)

build: gen
	cd $(GEN_DIR) && $(CARGO) build --workspace --locked

release: gen
	cd $(GEN_DIR) && $(CARGO) build --release -p $(GEN_BIN) --locked
	@/bin/ls -lh $(GEN_DIR)/target/release/

run: gen
	cd $(GEN_DIR) && $(CARGO) run -p $(GEN_BIN)

test: gen
	cd $(GEN_DIR) && $(CARGO) test --workspace --locked

# 模板源码里的 `{{...}}` 让 rustfmt 无法解析，所以格式化在生成目录里做，再把
# 结果按占位符反向替换回模板（只回写模板里已有的文件，见脚本注释）
fmt: gen
	cd $(GEN_DIR) && $(CARGO) fmt --all
	python3 $(ROOT)/scripts/template-sync.py --gen-dir $(GEN_DIR) \
		--project-name $(GEN_NAME) --crate-prefix $(GEN_PREFIX)

fmt-check: gen
	cd $(GEN_DIR) && $(CARGO) fmt --all -- --check

# fmt-check 只验一组名字。rustfmt 的排版取决于**替换之后**的行宽与排序，所以
# 「对 tpl-project/tplx 是 fmt-clean」不蕴含「对任意名字都是」。这一条静态扫模板源码
# 本身，不需要生成目录，因此放在 check 的最前面——它最快，也最可能是失败的那个
fmt-portable:
	python3 $(ROOT)/scripts/check-fmt-portability.py

lint: gen
	cd $(GEN_DIR) && $(CARGO) clippy --workspace --all-targets --locked -- -D warnings

coverage: gen
	mkdir -p $(ROOT)/coverage
	cd $(GEN_DIR) && $(CARGO) llvm-cov --workspace --locked --lcov --output-path $(ROOT)/coverage/lcov.info

# fmt-portable 只建模 rustfmt 的 100 列上限，抓不到 fn_call_width / chain_width 这类
# 60 列的子阈值（`matches!(app, <prefix>_core::AppError::Storage(_))` 就是栽在后者上）。
# 最后一道防线只能是真的换一组名字再跑一遍 rustfmt。名单里必须留一组极端长的名字：
# 短名字彼此差异太小，翻不出边界。不编译，每组只花一次展开 + 一次 rustfmt
FMT_MATRIX ?= ab:zzz payments:pay acme-billing-service:acme \
	a-very-long-project-name-here:supercalifragilistic
MATRIX_ROOT := $(GEN_ROOT)/matrix
fmt-matrix:
	@set -e; for pair in $(FMT_MATRIX); do \
		name=$${pair%%:*}; prefix=$${pair##*:}; \
		printf '  fmt-matrix %-30s %-22s ... ' "$$name" "$$prefix"; \
		m="$(MAKE) --no-print-directory fmt-check GEN_ROOT=$(MATRIX_ROOT) GEN_NAME=$$name GEN_PREFIX=$$prefix"; \
		if $$m >/dev/null 2>&1; then echo ok; \
		else echo FAIL; $$m 2>&1 | grep -E '^Diff in' || true; exit 1; fi; \
	done; rm -rf $(MATRIX_ROOT)

check: fmt-portable fmt-check fmt-matrix lint test

# 模板自检：cargo-generate 自己的 `--test`，把本仓库当模板展开到它的临时目录再跑测试。
# 与 `make check` 不重复的部分有三处：换一组名字（抓只在 tpl-project/tplx 这组下才对的
# 占位符）、不复用 GEN_DIR 的 target/（从零编译，抓「只在增量下才绿」）、走的是
# cargo-generate 自己的展开路径而不是 Makefile 拼的那条命令行。
# --destination 指到树外：不给它的话它会在 CWD 里留一个随机名的空目录
VERIFY_NAME ?= gt-project
VERIFY_PREFIX ?= gtx
VERIFY_DIR := $(GEN_ROOT)/verify
verify:
	rm -rf $(VERIFY_DIR)
	mkdir -p $(VERIFY_DIR)
	cd $(ROOT) && CARGO_GENERATE_TEST_CMD="$(CARGO) test --workspace --locked" \
	$(CARGO) generate --silent --name $(VERIFY_NAME) --define crate_prefix=$(VERIFY_PREFIX) \
		--destination $(VERIFY_DIR) --test
	rm -rf $(VERIFY_DIR)

# Cargo.lock 随模板一起发布（`--locked` 门禁的前提）。锁文件里只有包名，反向
# 替换只需要处理 `<prefix>-`
lock: gen
	cd $(GEN_DIR) && $(CARGO) generate-lockfile
	sed -e 's/$(GEN_PREFIX)-/{{crate_prefix}}-/g' $(GEN_DIR)/Cargo.lock > $(ROOT)/Cargo.lock

clean:
	rm -rf $(GEN_DIR)
