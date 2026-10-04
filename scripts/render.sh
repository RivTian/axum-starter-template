#!/bin/bash
# Renders template combinations from scripts/matrix.tsv with a given cargo-generate.
#
# usage: scripts/render.sh [--cg BIN] [--template-root DIR] [--out DIR] [ID...]
#
# With no ID every combination is rendered. Each one lands in OUT/ID/NAME. The inputs are
# fixed so that renders are reproducible: every placeholder, the author and the year come
# from the command line, and cargo-generate runs with an empty CARGO_HOME so that user-level
# cargo-generate settings cannot leak in. --template-root renders another checkout (for
# example a worktree of an older commit) instead of this one.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cg="${CG:-cargo-generate}"
root="$(cd "$here/.." && pwd)"
out="${RENDER_DIR:-${TMPDIR:-/tmp}/rs-starter-template-render}"

while [ $# -gt 0 ]; do
  case "$1" in
    --cg) cg="$2"; shift 2 ;;
    --template-root) root="$(cd "$2" && pwd)"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    --) shift; break ;;
    -*) echo "render.sh: unknown option $1" >&2; exit 2 ;;
    *) break ;;
  esac
done

matrix="$root/scripts/matrix.tsv"
[ -f "$matrix" ] || { echo "render.sh: $matrix not found" >&2; exit 2; }
if [ $# -eq 0 ]; then
  ids=()
  while IFS=$'\t' read -r id _; do
    if [ -n "$id" ] && [ "$id" != id ]; then ids+=("$id"); fi
  done <"$matrix"
  set -- "${ids[@]}"
fi

version="$("$cg" --version 2>/dev/null || true)"
[ -n "$version" ] || { echo "render.sh: cannot run $cg" >&2; exit 2; }
mkdir -p "$out"
empty_home="$(mktemp -d)"
trap 'rm -rf "$empty_home"' EXIT

status=0
for id in "$@"; do
  line="$(awk -F'\t' -v id="$id" '$1 == id' "$matrix")"
  [ -n "$line" ] || { echo "render.sh: unknown combination $id" >&2; exit 2; }
  name="$(printf '%s' "$line" | cut -f2)"
  license="$(printf '%s' "$line" | cut -f3)"
  with_docker="$(printf '%s' "$line" | cut -f4)"
  with_ci="$(printf '%s' "$line" | cut -f5)"
  dest="$out/$id"
  rm -rf "$dest"
  mkdir -p "$dest"
  if CARGO_HOME="$empty_home" "$cg" generate --path "$root" --name "$name" \
      --destination "$dest" --silent --vcs none --no-workspace \
      -d license="$license" -d with_docker="$with_docker" -d with_ci="$with_ci" \
      -d authors="Template CI <ci@example.invalid>" -d year=2026 >"$dest.log" 2>&1; then
    echo "rendered $id ($name) with $version into $dest/$name"
  else
    echo "render.sh: $id failed with $version; log follows" >&2
    cat "$dest.log" >&2
    status=1
  fi
done
exit "$status"
