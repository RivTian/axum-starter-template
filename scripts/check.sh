#!/bin/bash
# Quality gate for rendered combinations, run outside the template repository.
#
# usage: scripts/check.sh [--render-dir DIR] [ID...]
#
# With no ID every combination in scripts/matrix.tsv is checked. Each combination is copied
# from the render directory (see scripts/render.sh) to a fresh temporary directory, made a
# git repository, and checked with the generated project's own `just check`.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
matrix="$here/matrix.tsv"
render_dir="${RENDER_DIR:-${TMPDIR:-/tmp}/rs-starter-template-render}"

while [ $# -gt 0 ]; do
  case "$1" in
    --render-dir) render_dir="$2"; shift 2 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    --) shift; break ;;
    -*) echo "check.sh: unknown option $1" >&2; exit 2 ;;
    *) break ;;
  esac
done
if [ $# -eq 0 ]; then
  ids=()
  while IFS=$'\t' read -r id _; do
    if [ -n "$id" ] && [ "$id" != id ]; then ids+=("$id"); fi
  done <"$matrix"
  set -- "${ids[@]}"
fi

run() {
  # Prints the step, runs it, and shows its output only when it fails.
  local log
  log="$(mktemp)"
  printf '  %-58s ' "$*"
  if "$@" >"$log" 2>&1; then
    echo ok
    rm -f "$log"
  else
    echo FAILED
    tail -n 60 "$log"
    rm -f "$log"
    return 1
  fi
}

status=0
for id in "$@"; do
  name="$(awk -F'\t' -v id="$id" '$1 == id { print $2 }' "$matrix")"
  [ -n "$name" ] || { echo "check.sh: unknown combination $id" >&2; exit 2; }
  src="$render_dir/$id/$name"
  [ -d "$src" ] || { echo "check.sh: $src does not exist; run scripts/render.sh first" >&2; exit 2; }
  work="$(mktemp -d)"
  /bin/cp -R "$src" "$work/$name"
  echo "== $id ($name) in $work/$name"
  # A repository with one commit, so that the project knows its commit. Signing is off: a
  # maintainer's commit.gpgsign must not make the check depend on a key.
  # `set -e` does not apply inside an `if` condition, so the steps are chained with &&.
  if ! (
    cd "$work/$name" &&
      git init -q &&
      git add -A &&
      git -c user.name=check -c user.email=check@example.invalid -c commit.gpgsign=false \
        commit -q -m check &&
      run just check
  ); then
    status=1
  fi
  rm -rf "$work"
done
exit "$status"
