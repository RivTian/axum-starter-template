#!/bin/bash
# Quality gate for rendered combinations, run outside the template repository.
#
# usage: scripts/check.sh [--render-dir DIR] [--full] [ID...]
#
# With no ID every combination in scripts/matrix.tsv is checked. Each combination is copied
# from the render directory (see scripts/render.sh) to a fresh temporary directory and made
# a git repository with one commit. The default checks it with the generated project's own
# `just check`. --full also runs, with --locked once the lock file exists: smoke runs of
# `check-config` and `run`, `just msrv`, `just console-check`, `just package` and the
# archive's contents, the lock-file errors of CI and of the image build, and for
# combinations with a Dockerfile the image build and a smoke test. Images are tagged
# rs-starter-template-check-<name>; no other image is created or removed.
# The checks below are called through run() and step(), which shellcheck cannot follow.
# shellcheck disable=SC2329
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
matrix="$here/matrix.tsv"
render_dir="${RENDER_DIR:-${TMPDIR:-/tmp}/rs-starter-template-render}"
mode=quick

while [ $# -gt 0 ]; do
  case "$1" in
    --render-dir) render_dir="$2"; shift 2 ;;
    --full) mode=full; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
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

# The functions below run as `if` conditions inside `run`, where `set -e` does not apply, so
# every step returns on failure explicitly. Readers in pipes read to the end (`grep` to
# /dev/null, `sed -n 1p`): `grep -q` and `head` stop early, and under pipefail the writer's
# SIGPIPE would then fail a check that matched.

check_config() {
  # `just check-config` passes and prints the table.
  local out
  out="$(just check-config 2>/dev/null)" || return 1
  [ "$(printf '%s\n' "$out" | sed -n 1p)" = "configuration is valid" ] || { printf '%s\n' "$out"; return 1; }
}

serves() {
  # $1: the project name, $2: the variable prefix. Starts `just run` on a free port, waits
  # until it listens, checks /readyz, sends SIGTERM to the binary, and requires exit code 0
  # and `stopped` as the last log line.
  local name="$1" prefix="$2" log just_pid addr pid
  log="$(mktemp)" || return 1
  env "${prefix}_SERVER__HTTP_ADDR=127.0.0.1:0" "${prefix}_LIFECYCLE__DRAIN_DELAY=0s" \
    "${prefix}_LOG__FORMAT=json" just run >"$log" 2>&1 &
  just_pid=$!
  for _ in $(seq 900); do
    grep -q '"message":"listening"' "$log" && break
    kill -0 "$just_pid" 2>/dev/null || break
    sleep 1
  done
  addr="$(sed -n 's/.*"message":"listening","http.addr":"\([^"]*\)".*/\1/p' "$log" | sed -n 1p)"
  [ -n "$addr" ] || { tail -n 20 "$log"; kill "$just_pid" 2>/dev/null; return 1; }
  curl -fsS "http://$addr/readyz" >/dev/null || { kill "$just_pid"; return 1; }
  pid="$(pgrep -f "debug/$name run --config config/example.toml" | sed -n 1p)"
  [ -n "$pid" ] || { kill "$just_pid"; return 1; }
  kill -TERM "$pid" || return 1
  wait "$just_pid" || { tail -n 5 "$log"; return 1; }
  grep '"message"' "$log" | tail -n 1 | grep '"message":"stopped","exit_code":0' >/dev/null || { tail -n 5 "$log"; return 1; }
  rm -f "$log"
}

archive_holds() {
  # The release archive holds the binary, README.md, THIRD_PARTY_NOTICES.md and LICENSE
  # exactly when the project has one, and its SHA-256 file checks it.
  local name="$1" archive actual expected
  archive="$(ls dist/"$name"-*.tar.gz)" || return 1
  actual="$(tar -tzf "$archive" | sed 's#^[^/]*/##' | grep -v '^$' | sort | tr '\n' ' ')" || return 1
  expected="$( { printf '%s\n' "$name" README.md THIRD_PARTY_NOTICES.md; if [ -f LICENSE ]; then echo LICENSE; fi; } | sort | tr '\n' ' ')"
  [ "$actual" = "$expected" ] || { echo "archive: $actual; expected: $expected"; return 1; }
  # The checksum next to the archive verifies it.
  (cd dist && shasum -a 256 -c "$(basename "$archive").sha256") || return 1
}

ci_needs_lock() {
  # The CI step that requires Cargo.lock fails, with its message, in a copy without one.
  local step out
  step="$(grep -m 1 'test -f Cargo.lock' .github/workflows/ci.yml | sed 's/^ *//')" || return 1
  if out="$(bash -c "$step" 2>&1)"; then echo "passed without Cargo.lock"; return 1; fi
  printf '%s\n' "$out" | grep 'Cargo.lock is not committed' >/dev/null || { printf '%s\n' "$out"; return 1; }
}

image_needs_lock() {
  # The image build fails, with its message, in a copy without Cargo.lock.
  local image="$1" out
  if out="$(docker build -t "$image:nolock" . 2>&1)"; then
    docker image rm "$image:nolock" >/dev/null
    echo "built without Cargo.lock"
    return 1
  fi
  printf '%s\n' "$out" | grep 'Cargo.lock is missing' >/dev/null || { printf '%s\n' "$out" | tail -n 20; return 1; }
}

image_build() {
  # The image, built as `just docker-build` builds it, under the check's own tag.
  local image="$1" sha
  sha="$(git rev-parse --verify --quiet HEAD || echo unknown)"
  docker build --build-arg GIT_SHA="$sha" -t "$image:latest" .
}

image_serves() {
  # Healthy, ready, the commit as git SHA, the license texts in /usr/share/doc, and exit code
  # 0 after SIGTERM; the container and the image are removed afterwards.
  local name="$1" image="$2" container="$2-run" port sha notices status=0
  sha="$(git rev-parse HEAD)" || return 1
  docker run -d --name "$container" -p 127.0.0.1::8080 "$image:latest" >/dev/null || return 1
  {
    port="$(docker port "$container" 8080 | sed -n '1s/.*://p')"
    for _ in $(seq 60); do
      [ "$(docker inspect -f '{{.State.Health.Status}}' "$container")" = healthy ] && break
      sleep 1
    done
    [ "$(docker inspect -f '{{.State.Health.Status}}' "$container")" = healthy ] || { docker logs "$container" 2>&1 | tail -n 10; false; }
  } && curl -fsS "http://127.0.0.1:$port/readyz" >/dev/null \
    && { docker logs "$container" 2>&1 | grep -F "\"git_sha\":\"$sha\"" >/dev/null || { echo "no git_sha $sha in the log"; false; }; } \
    && notices="$(mktemp -d)" \
    && docker cp "$container:/usr/share/doc/$name/." "$notices/" \
    && cmp "$notices/THIRD_PARTY_NOTICES.md" THIRD_PARTY_NOTICES.md \
    && { if [ -f LICENSE ]; then cmp "$notices/LICENSE" LICENSE; else [ ! -e "$notices/LICENSE" ]; fi; } \
    && docker stop --time 30 "$container" >/dev/null \
    && [ "$(docker inspect -f '{{.State.ExitCode}}' "$container")" = 0 ] || status=1
  docker rm -f "$container" >/dev/null 2>&1
  docker image rm "$image:latest" >/dev/null 2>&1
  return "$status"
}

step() {
  # A step of the full check: runs it, and remembers a failure in `failed`.
  run "$@" || failed=1
}

full() {
  # $1: the project name, $2: the directory of this copy.
  local name="$1" dir="$2" prefix nolock image="rs-starter-template-check-$1" failed=0
  prefix="$(sed -n 's/^pub(crate) const ENV_PREFIX: &str = "\(.*\)";$/\1/p' "crates/$name/src/names.rs")"
  step just --list
  # No lock file yet: the recipes must work without --locked, and create it.
  step just check
  step test -f Cargo.lock
  git add Cargo.lock && git -c user.name=check -c user.email=check@example.invalid \
    -c commit.gpgsign=false commit -q -m lock
  export CARGO_LOCKED=--locked
  step check_config
  step serves "$name" "$prefix"
  step just msrv
  step just console-check
  step just package
  step archive_holds "$name"
  # With CARGO_TARGET_DIR set, the recipe must still find the binary.
  step env CARGO_TARGET_DIR="$PWD/target/elsewhere" just package
  nolock="$(mktemp -d)"
  /bin/cp -R "$dir" "$nolock/"
  (
    cd "$nolock/$name" || exit 1
    rm -rf Cargo.lock target dist
    if [ -f .github/workflows/ci.yml ]; then run ci_needs_lock || exit 1; fi
    if [ -f Dockerfile ]; then run image_needs_lock "$image" || exit 1; fi
  ) || failed=1
  rm -rf "$nolock"
  if [ -f Dockerfile ]; then
    step image_build "$image"
    step image_serves "$name" "$image"
  fi
  return "$failed"
}

status=0
for id in "$@"; do
  name="$(awk -F'\t' -v id="$id" '$1 == id { print $2 }' "$matrix")"
  [ -n "$name" ] || { echo "check.sh: unknown combination $id" >&2; exit 2; }
  src="$render_dir/$id/$name"
  [ -d "$src" ] || { echo "check.sh: $src does not exist; run scripts/render.sh first" >&2; exit 2; }
  work="$(mktemp -d)"
  /bin/cp -R "$src" "$work/$name"
  echo "== $id ($name, $mode) in $work/$name"
  # A repository with one commit, so that the project knows its commit. Signing is off: a
  # maintainer's commit.gpgsign must not make the check depend on a key. `set -e` does not
  # apply inside an `if` condition, so the steps are chained with &&.
  if ! (
    cd "$work/$name" &&
      git init -q &&
      git add -A &&
      git -c user.name=check -c user.email=check@example.invalid -c commit.gpgsign=false \
        commit -q -m check &&
      if [ "$mode" = full ]; then full "$name" "$work/$name"; else run just check; fi
  ); then
    status=1
  fi
  rm -rf "$work"
done
exit "$status"
