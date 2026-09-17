#!/usr/bin/env bash
# 前缀形状夹具 + 名字矩阵（只跑 fmt-check，不拉依赖）：
#   1) 前缀夹具：ok 必须生成成功；fail 必须被 pre hook 拒绝且消息指向 crate_prefix
#   2) names.tsv 里 mode=fmt 的行：生成 + `cargo fmt --check`
# 完整 make check 的矩阵见 scripts/check-generated.sh（mode=full）。
#
# 每个生成目录都在仓库树外，各自用自己的 target，不复用编译缓存。
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/matrix.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

say() { printf '  matrix: %s\n' "$*"; }

fail=0

# cargo-generate 的 --destination 要求目录已存在，这里统一用"建一个空目录 + cd 进去用 --name 生成"。
generate() {
    local name="$1" prefix="$2" dest="$3"
    mkdir -p "$dest"
    (cd "$dest" && cargo generate --path "$REPO" --name "$name" --define "crate_prefix=$prefix" 2>&1)
}

# ── 1) 前缀夹具 ────────────────────────────────────────────────────────────────
while IFS=$'\t' read -r expect prefix note; do
    case "$expect" in ''|'#'*) continue ;; esac
    dest="$WORK/prefix-$prefix"
    if output="$(generate fixture-service "$prefix" "$dest")"; then
        dest="$dest/fixture-service"
        if [ "$expect" = "ok" ]; then
            say "ok   prefix=$prefix ($note)"
        else
            say "FAIL prefix=$prefix 本应被拒绝（${note}）"
            fail=1
        fi
    else
        rm -rf "$dest"   # cargo-generate 在钩子失败前就把目标目录建好了
        if [ "$expect" = "fail" ]; then
            if printf '%s' "$output" | grep -q 'crate_prefix'; then
                say "ok   prefix=$prefix 被拒绝且消息指向 crate_prefix（${note}）"
            else
                say "FAIL prefix=$prefix 被拒了，但消息里没有 crate_prefix：$(printf '%s' "$output" | tail -2 | tr '\n' ' ')"
                fail=1
            fi
        else
            say "FAIL prefix=$prefix 本应生成成功（${note}）：$(printf '%s' "$output" | tail -2 | tr '\n' ' ')"
            fail=1
        fi
    fi
done < "$REPO/scripts/prefix-fixtures.tsv"

# ── 2) 名字矩阵 ────────────────────────────────────────────────────────────────
while IFS=$'\t' read -r mode label name prefix; do
    case "$mode" in ''|'#'*) continue ;; esac
    [ "$mode" = "fmt" ] || continue
    dest="$WORK/name-$label"
    if ! generate "$name" "$prefix" "$dest" >/dev/null; then
        say "FAIL 生成 ${label}（$name / ${prefix}）"
        fail=1
        continue
    fi
    dest="$dest/$name"
    (
        cd "$dest"
        export CARGO_TARGET_DIR="$WORK/target-$label"
        cargo fmt --all -- --check
    ) || { say "FAIL ${label}（$name / ${prefix}）在门禁里红了"; fail=1; continue; }
    say "ok   ${label}（$name / ${prefix}，mode=${mode}）"
done < "$REPO/scripts/names.tsv"

[ "$fail" -eq 0 ] || exit 1
say "OK"
