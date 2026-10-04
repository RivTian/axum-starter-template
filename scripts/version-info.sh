#!/bin/bash
# Version information of the generated binary in six git scenarios, on the rendered m2
# combination.
#
# usage: scripts/version-info.sh [--render-dir DIR]
#
# Each scenario copies the rendered project, without target/ and .git, to a directory of its
# own and builds the binary 2 or 3 times around git operations; every build must succeed
# without warnings, and `<name> --version` must show the expected short SHA or `unknown`.
# Builds share one target directory: $CARGO_TARGET_DIR/version-info when CARGO_TARGET_DIR
# is set, otherwise one inside the temporary work directory.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
render_dir="${RENDER_DIR:-${TMPDIR:-/tmp}/rs-starter-template-render}"
id=m2

while [ $# -gt 0 ]; do
  case "$1" in
    --render-dir) render_dir="$2"; shift 2 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "version-info.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

name="$(awk -F'\t' -v id="$id" '$1 == id { print $2 }' "$here/matrix.tsv")"
src="$render_dir/$id/$name"
[ -d "$src" ] || { echo "version-info.sh: $src does not exist; run scripts/render.sh first" >&2; exit 2; }
version="$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$src/Cargo.toml" | sed -n 1p)"
[ -n "$version" ] || { echo "version-info.sh: no workspace version in $src/Cargo.toml" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
target="${CARGO_TARGET_DIR:-$work/target}/version-info"
export CARGO_TARGET_DIR="$target"
# Git without the machine's configuration (signing, hooks, templates), with a fixed identity.
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=version-info GIT_AUTHOR_EMAIL=version-info@example.invalid
export GIT_COMMITTER_NAME=version-info GIT_COMMITTER_EMAIL=version-info@example.invalid
unset VERGEN_GIT_SHA

failures=0

project() {
  # $1: a new directory holding a copy of the rendered project without target/ and .git.
  /bin/cp -R "$src" "$1"
  rm -rf "$1/target" "$1/.git"
}

commit() {
  # $1: the repository, $2: the message; changes README.md so that there is something to commit.
  echo "$2" >>"$1/README.md"
  git -C "$1" add -A
  git -C "$1" commit -q -m "$2"
}

check() {
  # $1: the directory to build in, $2: the label, $3: the expected short SHA, `unknown`, or
  # `head` for the short SHA of HEAD in $1.
  local dir="$1" label="$2" expected="$3" log warnings actual
  log="$work/build.log"
  if [ "$expected" = head ]; then expected="$(git -C "$dir" rev-parse --short=7 HEAD)"; fi
  if ! (cd "$dir" && cargo build -p "$name") >"$log" 2>&1; then
    printf '  %-6s %-38s build failed\n' FAILED "$label"
    tail -n 20 "$log"
    failures=$((failures + 1))
    return 0
  fi
  # awk rather than grep: no match must not end the script under `set -e` and pipefail.
  warnings="$(awk '/^warning/ { n++ } END { print n + 0 }' "$log")"
  actual="$("$target/debug/$name" --version 2>&1)" || actual="exit status $?: $actual"
  if [ "$actual" = "$name $version ($expected)" ] && [ "$warnings" = 0 ]; then
    printf '  %-6s %-38s %-34s warnings=%s\n' ok "$label" "$actual" "$warnings"
  else
    printf '  %-6s %-38s %-34s warnings=%s, expected %s\n' FAILED "$label" "$actual" "$warnings" "$name $version ($expected)"
    awk '/^warning/ && n < 10 { print; n++ }' "$log"
    failures=$((failures + 1))
  fi
}

echo "S1 git init, no commit yet, then commits"
d="$work/s1"; project "$d"; git -C "$d" init -q -b main
check "$d" "no commit yet" unknown
git -C "$d" add -A; git -C "$d" commit -q -m first
check "$d" "after the first commit" head
commit "$d" second
check "$d" "after a second commit" head

echo "S2 packed refs"
d="$work/s2"; project "$d"; git -C "$d" init -q -b main; git -C "$d" add -A; git -C "$d" commit -q -m c1
check "$d" "at c1" head
git -C "$d" pack-refs --all
check "$d" "after pack-refs" head
commit "$d" c2
check "$d" "a commit after pack-refs" head

echo "S3 linked worktree"
d="$work/s3"; project "$d"; git -C "$d" init -q -b main; git -C "$d" add -A; git -C "$d" commit -q -m c1
git -C "$d" worktree add -q -b feature "$work/s3-worktree"
check "$work/s3-worktree" "a build in the worktree" head
commit "$work/s3-worktree" w1
check "$work/s3-worktree" "a commit in the worktree" head

echo "S4 commits"
d="$work/s4"; project "$d"; git -C "$d" init -q -b main; git -C "$d" add -A; git -C "$d" commit -q -m c1
check "$d" "at c1" head
commit "$d" c2
check "$d" "after c2" head

echo "S5 no .git (a source archive)"
d="$work/s5"; project "$d"
check "$d" "first build" unknown
check "$d" "rebuild" unknown

echo "S6 VERGEN_GIT_SHA overrides git"
d="$work/s6"; project "$d"; git -C "$d" init -q -b main; git -C "$d" add -A; git -C "$d" commit -q -m c1
VERGEN_GIT_SHA=0123456789abcdef0123456789abcdef01234567 check "$d" "a build with the override" 0123456
check "$d" "then without it" head
VERGEN_GIT_SHA=fedcba9876543210fedcba9876543210fedcba98 check "$d" "set after a build" fedcba9

if [ "$failures" -gt 0 ]; then
  echo "version-info.sh: $failures of 15 checks failed"
  exit 1
fi
echo "version-info.sh: all 15 checks passed"
