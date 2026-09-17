#!/usr/bin/env bash
# README 两条切片"照抄走通"的可重复证据：
#   task 切片：生成 → 应用 → make check → 起进程断言 flush tick / stopped
#   repo 切片：生成 → 应用（含迁移吃进编译期）→ make check → 起进程断言 notes written / stopped
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/slices.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

say() { printf '  slices: %s\n' "$*"; }

run_slice() {
    local which="$1" name="$2" prefix="$3"
    local dest="$WORK/$which"
    say "生成 ${name}（prefix=${prefix}）"
    mkdir -p "$dest"
    (cd "$dest" && cargo generate --path "$REPO" --name "$name" --define "crate_prefix=$prefix" >/dev/null)
    dest="$dest/$name"

    say "照 README 应用切片：$which"
    "$REPO/scripts/apply-slices.sh" "$dest" "$which"

    cd "$dest"
    export CARGO_TARGET_DIR="$WORK/target-$which"
    say "make check"
    make check >/dev/null

    local bin="$CARGO_TARGET_DIR/debug/${name//-/_}"
    local log="$WORK/$which.log"
    "$bin" >"$log" 2>&1 &
    local pid=$!
    sleep 4   # 等任务跑起来（flush 每 5s 一跳；notes-writer 立刻写）
    kill -INT "$pid"
    local waited=0
    while kill -0 "$pid" 2>/dev/null; do
        [ "$waited" -ge 300 ] && { kill -9 "$pid" 2>/dev/null || true; say "FAIL: 关停超时"; return 1; }
        sleep 0.1
        waited=$((waited + 1))
    done
    wait "$pid" || { cat "$log"; say "FAIL: 退出码非 0"; return 1; }

    case "$which" in
        task)
            grep -q "stopped task=flush runtime=main" "$log" \
                || { cat "$log"; say "FAIL: 新任务面没有 stopped 记录"; return 1; }
            ;;
        repo)
            grep -q "notes written" "$log" || { cat "$log"; say "FAIL: 仓储没有被消费（没有 notes written）"; return 1; }
            grep -q "stopped task=notes-writer runtime=main" "$log" \
                || { cat "$log"; say "FAIL: 仓储消费者没有 stopped 记录"; return 1; }
            grep -q "migrations applied" "$log" || { cat "$log"; say "FAIL: 迁移没有跑"; return 1; }
            ;;
    esac
    say "ok   ${which}：make check 绿 + 进程内行为符合预期"
}

run_slice task slice-task svc
run_slice repo slice-repo svc
say "OK"
