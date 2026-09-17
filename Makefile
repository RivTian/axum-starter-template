# 模板仓库自己的门禁。
#
# 生成结果拿到的是 Makefile.liquid 渲染出来的那份（三条目标：fmt-check / lint / test）。
# 这里的目标只在模板仓库里跑：审计渲染面、跑名字矩阵、起真实进程、照抄走通两条切片。
#
# 需要：make、cargo、cargo-generate 0.24.x。审计工具自己的 target 放在仓库外，
# 免得在模板树里留下构建产物（顶层白名单会拦）。
.PHONY: gate audit slices-check template-fmt matrix check-gen probe slices clean

AUDIT_TARGET := $(if $(TMPDIR),$(TMPDIR),/tmp)/axum-starter-template-audit
AUDIT := CARGO_TARGET_DIR=$(AUDIT_TARGET) cargo run --quiet --manifest-path tools/audit/Cargo.toml --

# 全量门禁。按依赖顺序：先静态审计（快、失败早），再生成矩阵，最后真实进程与切片。
gate: audit slices-check template-fmt matrix check-gen probe slices
	@echo "gate: all green"

audit: ## 模板树审计：顶层白名单、垃圾/符号链接、ignore 路径、占位符、`.liquid` 名单
	$(AUDIT) template $(CURDIR)

slices-check: ## README 里的切片代码块必须与 scripts/slices/ 的答案文件逐字一致
	$(AUDIT) slices $(CURDIR)

template-fmt: ## 审计工具自身的格式
	cargo fmt --manifest-path tools/audit/Cargo.toml -- --check

matrix: ## 前缀形状夹具 + fmt-only 的名字矩阵（快，不拉依赖）
	bash scripts/matrix.sh

check-gen: ## 极端长名 / 极端短名各一次完整 make check + 生成结果审计（慢）
	bash scripts/check-generated.sh

probe: ## 真实进程探针：构建串、路径锚点、SIGINT 关停、退出码
	bash scripts/probe.sh

slices: ## 两条 README 切片照抄走通：task（新任务面）/ repo（新仓储）
	bash scripts/slices.sh

clean:
	rm -rf $(AUDIT_TARGET)
