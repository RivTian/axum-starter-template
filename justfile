# Maintainer recipes of the template repository. Combinations come from scripts/matrix.tsv.

set shell := ["bash", "-euo", "pipefail", "-c"]

render_dir := env("RENDER_DIR", env("TMPDIR", "/tmp") + "/rs-starter-template-render")
cg := env("CG", "cargo-generate")

# List the recipes.
default:
    @just --list

# Template lint and its self-test, the vendored files, shellcheck, and actionlint.
lint:
    scripts/template-lint.py
    scripts/template-lint.py --self-test
    scripts/vendor-pingora-error.py --check-patches
    shellcheck scripts/*.sh
    if [ -d .github/workflows ]; then actionlint; fi

# Render the given combinations, or all of them.
render *ids:
    scripts/render.sh --cg "{{cg}}" --out "{{render_dir}}" {{ids}}

# Render and check the given combinations, or all of them, with the generated `just check`.
check *ids: (render ids)
    scripts/check.sh --render-dir "{{render_dir}}" {{ids}}

# Render and run the full check of the given combinations, or of all of them.
full *ids: (render ids)
    scripts/check.sh --full --render-dir "{{render_dir}}" {{ids}}

# The per-commit tier: lint, then render and check the default combination.
commit: lint (check "m2")

# Project-name rules and that nothing prompts without a terminal.
names:
    CG="{{cg}}" scripts/names.sh

# The binary's version information in six git scenarios, on the rendered m2 combination.
version-info: (render "m2")
    scripts/version-info.sh --render-dir "{{render_dir}}"

# Everything the template CI runs, in the same order.
ci: lint full names version-info
