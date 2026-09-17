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

python3 - "$REPO" "$WORK/$NAME" "$PREFIX" "$NAME" <<'PY'
import pathlib
import sys

repo = pathlib.Path(sys.argv[1])
generated = pathlib.Path(sys.argv[2])
prefix = sys.argv[3]
# crate_name 由 --name 派生（snake_case）；env_prefix 是它的 shouty snake 形态。
# 注意：不能用 prefix 推这两个名字，否则会把 {{env_prefix}} 反向映射错。
crate_name = sys.argv[4].replace("-", "_")
env_prefix = crate_name.upper()

name_map = {
    f"{prefix}_core": "{{crate_prefix_snake}}_core",
    f"{prefix}_config": "{{crate_prefix_snake}}_config",
    f"{prefix}_runtime": "{{crate_prefix_snake}}_runtime",
    f"{prefix}_storage": "{{crate_prefix_snake}}_storage",
    f"{prefix}_http": "{{crate_prefix_snake}}_http",
    f"{prefix}_app": "{{crate_prefix_snake}}_app",
    # 派生值可能是字符串字面量的一部分（例如 about = "dev-service 服务"），所以按裸文本替换。
    env_prefix: "{{env_prefix}}",
    crate_name.replace("_", "-"): "{{project-name}}",
    crate_name: "{{crate_name}}",
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

# 守卫：.rs 里必须出现这些占位符（crate_prefix 只出现在清单/文档里，不在此列）。
sources = [
    p
    for p in repo.rglob("*.rs")
    if not any(part in {"docs", "scripts", "tools", ".git", "target"} for part in p.parts)
]
texts = "\n".join(p.read_text() for p in sources)
missing = [
    name
    for name in ["{{crate_prefix_snake}}", "{{crate_name}}", "{{project-name}}", "{{env_prefix}}"]
    if name not in texts
]
if missing:
    raise SystemExit(f"反向映射之后模板里缺少占位符：{missing}（检查 name_map）")
print(f"formatted {written} .rs files back into the template")
PY
