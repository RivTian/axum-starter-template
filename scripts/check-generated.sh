#!/usr/bin/env bash
# 完整生成矩阵：names.tsv 里 mode=full 的行（极端长名 / 极端短名）各跑一次完整 `make check`，
# 并对生成结果做内容审计。生成目录在仓库树外，各自用自己的 target，不复用编译缓存。
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/check-gen.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

say() { printf '  check-gen: %s\n' "$*"; }
fail=0

while IFS=$'\t' read -r mode label name prefix; do
    case "$mode" in ''|'#'*) continue ;; esac
    [ "$mode" = "full" ] || continue
    dest="$WORK/$label"
    say "生成 ${label}：name=$name prefix=$prefix"
    mkdir -p "$dest"
    if ! (cd "$dest" && cargo generate --path "$REPO" --name "$name" --define "crate_prefix=$prefix" >/dev/null); then
        say "FAIL 生成 $label"
        fail=1
        continue
    fi
    dest="$dest/$name"
    (
        cd "$dest"
        export CARGO_TARGET_DIR="$WORK/target-$label"
        make check
        CARGO_TARGET_DIR="$WORK/audit-target" cargo run --quiet \
            --manifest-path "$REPO/tools/audit/Cargo.toml" -- generated "$dest"
    ) || { say "FAIL $label 在门禁里红了"; fail=1; continue; }
    say "ok   ${label}（完整 make check + 生成结果审计）"
done < "$REPO/scripts/names.tsv"

[ "$fail" -eq 0 ] || exit 1
say "OK"
