#!/bin/bash
# Checks the project-name rules and that nothing prompts without a terminal, on every input
# path, with the cargo-generate in CG (default: the one on PATH).
#
# usage: scripts/names.sh [--template-root DIR]
#
# Paths: --name (valid, invalid, reserved), CARGO_GENERATE_VALUE_PROJECT_NAME (valid, invalid)
# and the interactive prompt (driven by scripts/names.exp). A run without a terminal and
# without --silent must not prompt at all.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
while [ $# -gt 0 ]; do
  case "$1" in
    --template-root) root="$(cd "$2" && pwd)"; shift 2 ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "names.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

invalid_message() {
  echo "invalid project name '$1': use 2 to 64 characters: lowercase letters, digits and single hyphens, starting with a letter"
}
reserved_message() {
  echo "invalid project name '$1': the name is reserved"
}
dependency_message() {
  echo "invalid project name '$1': a dependency of the template has this name"
}

failures=0
fail() {
  echo "  FAILED: $*"
  failures=$((failures + 1))
}

# generate CG DEST [extra args...]: runs cargo-generate without a terminal, output in DEST.log.
generate() {
  local cg="$1" dest="$2"
  shift 2
  rm -rf "$dest"
  mkdir -p "$dest"
  set +e
  CARGO_HOME="$empty_home" "$cg" generate --path "$root" --destination "$dest" --vcs none \
    --no-workspace -d license=MIT -d with_docker=false -d with_ci=false \
    -d authors=Names -d year=2026 "$@" </dev/null >"$dest.log" 2>&1
  local status=$?
  set -e
  return "$status"
}

# expect_result NAME STATUS WANT_STATUS DEST WANT_DIRS [MESSAGE]
expect_result() {
  local label="$1" status="$2" want="$3" dest="$4" want_dirs="$5" message="${6:-}"
  local dirs
  dirs="$(cd "$dest" && find . -mindepth 1 -maxdepth 1 | sed 's|^\./||' | sort | tr '\n' ' ')"
  if [ "$status" -ne "$want" ]; then
    fail "$label: exit status $status, expected $want"
    tail -n 5 "$dest.log"
  elif [ "$dirs" != "$want_dirs" ]; then
    fail "$label: destination holds [$dirs], expected [$want_dirs]"
  elif [ -n "$message" ] && ! grep -qF "$message" "$dest.log"; then
    fail "$label: output lacks: $message"
    tail -n 5 "$dest.log"
  else
    echo "  ok: $label"
  fi
}

# 65 characters: one more than crates.io allows.
long_name="kappa-service-with-a-deliberately-long-name-to-reach-the-limit650"
work="$(mktemp -d)"
empty_home="$work/cargo-home"
mkdir -p "$empty_home"
trap 'rm -rf "$work"' EXIT

cg="${CG:-cargo-generate}"
version="$("$cg" --version)"
echo "== $version"
d="$work/run"

status=0; generate "$cg" "$d" --name qx-loc -d with_docker=true -d with_ci=true || status=$?
expect_result "no prompt without a terminal when every value is given" "$status" 0 "$d" "qx-loc "

# Each rule once: too short, leading digit, underscore, double hyphen, trailing hyphen, too long.
for name in x 9lives acme_svc a--b ab- "$long_name"; do
  status=0; generate "$cg" "$d" --silent --name "$name" || status=$?
  expect_result "--name $name is rejected before the directory exists" "$status" 1 "$d" "" "$(invalid_message "$name")"
done
for name in build com1; do
  status=0; generate "$cg" "$d" --silent --name "$name" || status=$?
  expect_result "--name $name is reserved" "$status" 1 "$d" "" "$(reserved_message "$name")"
done

for name in tokio http-body; do
  status=0; generate "$cg" "$d" --silent --name "$name" || status=$?
  expect_result "--name $name is a dependency's name" "$status" 1 "$d" "" "$(dependency_message "$name")"
done

status=0; CARGO_GENERATE_VALUE_PROJECT_NAME=qx-env generate "$cg" "$d" || status=$?
expect_result "CARGO_GENERATE_VALUE_PROJECT_NAME=qx-env is accepted" "$status" 0 "$d" "qx-env "

status=0; CARGO_GENERATE_VALUE_PROJECT_NAME=x generate "$cg" "$d" || status=$?
expect_result "CARGO_GENERATE_VALUE_PROJECT_NAME=x is rejected by the pre hook" "$status" 1 "$d" "x " "$(invalid_message x)"

rm -rf "$d"; mkdir -p "$d"
status=0
CARGO_HOME="$empty_home" expect "$here/names.exp" "$cg" "$root" "$d" >"$d.log" 2>&1 || status=$?
expect_result "prompt: x is rejected and asked again, qx-tty is accepted" "$status" 0 "$d" "qx-tty " "$(invalid_message x)"

if [ "$failures" -ne 0 ]; then
  echo "names.sh: $failures check(s) failed"
  exit 1
fi
echo "names.sh: all checks passed"
