#!/usr/bin/env bash
# 模板维护工具：模板源码（带 {{占位符}}）不是合法 Rust，rustfmt 没法直接跑。
# 这里先真实生成一份（固定 svc / dev_service），在生成副本上跑 cargo fmt，
# 再把格式化结果按占位符反向映射回模板源码。
#
# 只在模板仓库里用；不进生成结果（scripts/ 在 cargo-generate.toml 的 ignore 里）。
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${PREFIX:-svc}"
NAME="${NAME:-dev-service}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/format-template.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

cd "$WORK"
cargo generate --path "$REPO" --name "$NAME" --define "crate_prefix=$PREFIX" >/dev/null
cd "$NAME"
cargo fmt --all

python3 - "$REPO" "$WORK/$NAME" "$PREFIX" <<'PY'
import pathlib
import sys

repo = pathlib.Path(sys.argv[1])
generated = pathlib.Path(sys.argv[2])
prefix = sys.argv[3]
crate_name = sys.argv[3].replace("-", "_")

name_map = {
    f"{prefix}_core": "{{crate_prefix_snake}}_core",
    f"{prefix}_config": "{{crate_prefix_snake}}_config",
    f"{prefix}_runtime": "{{crate_prefix_snake}}_runtime",
    f"{prefix}_storage": "{{crate_prefix_snake}}_storage",
    f"{prefix}_http": "{{crate_prefix_snake}}_http",
    f"{prefix}_app": "{{crate_prefix_snake}}_app",
    f'"{crate_name.upper()}"': '"{{env_prefix}}"',
}

written = 0
for path in sorted(generated.rglob("*.rs")):
    relative = path.relative_to(generated)
    target = repo / relative
    if not target.exists():
        print(f"skip (not in template): {relative}")
        continue
    text = path.read_text()
    for generated_name, template_name in name_map.items():
        text = text.replace(generated_name, template_name)
    target.write_text(text)
    written += 1
print(f"formatted {written} .rs files back into the template")
PY
