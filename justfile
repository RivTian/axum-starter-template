# Maintainer recipes of the template repository. Combinations come from scripts/matrix.tsv.

set shell := ["bash", "-euo", "pipefail", "-c"]

render_dir := env("RENDER_DIR", env("TMPDIR", "/tmp") + "/rs-starter-template-render")
cg := env("CG", "cargo-generate")

# List the recipes.
default:
    @just --list

# Template lint and its self-test, and shellcheck.
lint:
    scripts/template-lint.py
    scripts/template-lint.py --self-test
    shellcheck scripts/*.sh

# Render the given combinations, or all of them.
render *ids:
    scripts/render.sh --cg "{{cg}}" --out "{{render_dir}}" {{ids}}

# Render and check the given combinations, or all of them, with the generated `just check`.
check *ids: (render ids)
    scripts/check.sh --render-dir "{{render_dir}}" {{ids}}

# The per-commit tier: lint, then render and check the default combination.
commit: lint (check "m2")
