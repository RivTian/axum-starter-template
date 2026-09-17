#!/usr/bin/env bash
# 把生成项目根 README 的两条垂直切片"照抄"进一个生成项目里（模板侧证据，不是给用户用的脚本）。
# 用法：apply-slices.sh <生成项目目录> <task|repo|both>
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SLICES="$REPO/scripts/slices"
PROJECT="${1:?用法: apply-slices.sh <生成项目目录> <task|repo|both>}"
WHICH="${2:-both}"

python3 "$REPO/scripts/apply_slices.py" "$PROJECT" "$SLICES" "$WHICH"
