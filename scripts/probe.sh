#!/usr/bin/env bash
# 真实进程探针（模板侧证据，不属于生成结果的门禁）：
# 生成 → 构建 → 从两个不同的 cwd 起进程 → SIGINT → 断言构建串/锚点/关停日志/退出码。
#
# 进程用 Python 起：bash 后台任务的 SIGINT 会被置为忽略，`kill -INT` 打不到真实进程。
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/probe.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

NAME="${PROBE_NAME:-probe-service}"
PREFIX="${PROBE_PREFIX:-probe}"
BIN_NAME="${NAME//-/_}"

say() { printf '  probe: %s\n' "$*"; }

cd "$WORK"
say "generate $NAME (prefix=$PREFIX)"
cargo generate --path "$REPO" --name "$NAME" --define "crate_prefix=$PREFIX" >/dev/null

cd "$NAME"
export CARGO_TARGET_DIR="$WORK/target"
say "build（fresh target dir，不复用任何编译缓存）"
cargo build --quiet

BIN="$CARGO_TARGET_DIR/debug/$BIN_NAME"
EXE_DIR="$(dirname "$BIN")"
mkdir -p "$WORK/cwd-a" "$WORK/cwd-b"

run_and_stop() {
    local cwd="$1" log="$2"
    python3 "$REPO/scripts/probe_run.py" "$BIN" "$cwd" "$log"
}

say "第一次：cwd=$WORK/cwd-a"
set +e
run_and_stop "$WORK/cwd-a" "$WORK/log-a.txt"
CODE_A=$?
set -e
[ "$CODE_A" -eq 0 ] || { cat "$WORK/log-a.txt"; say "FAIL: 退出码 $CODE_A（应为 0）"; exit 1; }

grep -q 'build=' "$WORK/log-a.txt" || { cat "$WORK/log-a.txt"; say "FAIL: 第一条日志里没有构建串"; exit 1; }
say "构建串: $(grep -m1 -o 'build=[^,]*' "$WORK/log-a.txt" | cut -c1-90)"

grep -q "stopped task=http runtime=main" "$WORK/log-a.txt" \
    || { grep -n 'stopped' "$WORK/log-a.txt" || true; say "FAIL: 关停日志里没有 http 任务的 stopped（含 runtime）"; exit 1; }
grep -q "stopped task=config-watch runtime=main" "$WORK/log-a.txt" \
    || { grep -n 'stopped' "$WORK/log-a.txt" || true; say "FAIL: 关停日志里没有 config-watch 任务的 stopped（含 runtime）"; exit 1; }
say "关停日志: 每个任务都有 stopped（任务名 + runtime）"

[ -f "$EXE_DIR/config.toml" ] || { say "FAIL: 默认配置没有落在可执行文件所在目录（$EXE_DIR）"; exit 1; }
[ ! -f "$WORK/cwd-a/config.toml" ] || { say "FAIL: cwd 里出现了配置（锚点不应该是 cwd）"; exit 1; }
say "锚点: 配置落在 $EXE_DIR/config.toml，没有落进 cwd"

say "第二次：cwd=$WORK/cwd-b（同一二进制、不同工作目录）"
set +e
run_and_stop "$WORK/cwd-b" "$WORK/log-b.txt"
CODE_B=$?
set -e
[ "$CODE_B" -eq 0 ] || { cat "$WORK/log-b.txt"; say "FAIL: 第二次退出码 $CODE_B"; exit 1; }
LOGGED_CONFIG="$(grep -o 'path=[^ ]*' "$WORK/log-b.txt" | head -1 | cut -d= -f2)"
[ -n "$LOGGED_CONFIG" ] || { cat "$WORK/log-b.txt"; say "FAIL: 日志里没有 configuration loaded"; exit 1; }
SAME_DIR="$(cd "$(dirname "$LOGGED_CONFIG")" && pwd -P)"
EXPECTED_DIR="$(cd "$EXE_DIR" && pwd -P)"
[ "$SAME_DIR" = "$EXPECTED_DIR" ] \
    || { say "FAIL: 第二次读到的配置是 $LOGGED_CONFIG，不在锚点 $EXE_DIR 下"; exit 1; }
[ ! -f "$WORK/cwd-b/config.toml" ] || { say "FAIL: cwd-b 里出现了配置"; exit 1; }
say "换个目录启动读到同一份配置"

say "OK"
