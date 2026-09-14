# Adapted workflow from backup 1ac7584. Each invocation gets an isolated source
# directory outside this repository; no overwrite and no global source pointer.
PYTHON ?= python3
GEN_NAME ?= example-service
GEN_PREFIX ?= example
GEN_ROOT ?= $(HOME)/.cache/axum-starter-template/m1
DRIVER = $(PYTHON) scripts/template.py
ARGS = --name "$(GEN_NAME)" --prefix "$(GEN_PREFIX)" --gen-root "$(GEN_ROOT)"
.DEFAULT_GOAL := help
.PHONY: help gen check matrix verify fmt lock tooling-test clean

help:
	@echo 'Template: gen / check / matrix / verify / fmt / lock / tooling-test'
	@echo 'Each generation prints GENERATED_PROJECT; no source directory is overwritten.'
	@echo 'clean requires DIR=<printed managed generated project>; artifact cache is kept.'
gen:
	$(DRIVER) gen $(ARGS)
check:
	$(MAKE) tooling-test
	$(DRIVER) check $(ARGS)
	$(DRIVER) matrix --gen-root "$(GEN_ROOT)"
matrix:
	$(DRIVER) matrix --gen-root "$(GEN_ROOT)"
verify:
	$(DRIVER) verify --name clean-room-service --prefix verifyx --gen-root "$(GEN_ROOT)"
fmt:
	$(DRIVER) fmt $(ARGS)
lock:
	$(DRIVER) lock $(ARGS)
tooling-test:
	$(PYTHON) -m unittest discover -s scripts -p 'test_*.py' -v
clean:
	@test -n "$(DIR)" || (echo 'clean requires DIR=<managed generated project>'; exit 2)
	$(DRIVER) clean --directory "$(DIR)" --gen-root "$(GEN_ROOT)"
