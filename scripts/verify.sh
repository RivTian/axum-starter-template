#!/usr/bin/env bash
# 走 cargo-generate **自己的** `--test` 展开路径，用一个随机名字、在一棵空的 target 上，
# 跑一遍生成结果自己的 `make check`。
#
#   scripts/verify.sh [--quiet]
#
# ## 它和 `make check` 里那趟 `project-check` 有什么不一样
#
# 三处，每一处都是前面那趟**结构上做不到**的：
#
#   1. **另一条展开路径。** `check` 全程走 `--path`（gen）与 `--git`（verify-git）。
#      `--test` 是 cargo-generate 的第三条路：它把 `$CWD` 当模板原地展开，然后在展开出来
#      的那棵树里执行一条命令。前两条走的是"复制到别处再展开"，这条不是。
#   2. **名字不是门禁挑的。** `--test` 忽略 `--name`，项目名由 cargo-generate 自己的随机
#      词表给出（实测：`sour-sand`、`inquisitive-arch`）。名字矩阵那四组是**选出来的**边界，
#      选的人心里有假设；随机名字没有假设。每次运行换一个，一年下来覆盖面是矩阵给不了的。
#   3. **空的 target。** `check` 里所有 cargo 调用共用 `$GATE_ROOT/cache`，那是刻意的——
#      不共用的话一条门禁要编十几分钟。共用的代价是"第一次构建能不能成"从来没被验过：
#      缓存里可能躺着一份用别的 feature 组合编出来的依赖，而它恰好让某条本该红的
#      feature 闭包变绿。这一趟不设 `CARGO_TARGET_DIR`，从零编。
#
# 所以它是 `check` 之外的独立入口，而不是 `check` 的一个子目标：它慢（冷编译整棵依赖树），
# 慢到挂进 `check` 会让人开始躲着 `check` 走，而一个被躲开的门禁等于没有。
#
# ## 为什么要先把模板树复制一份
#
# `--test` 展开的是 `$CWD`，没法改；而它**会在 `$CWD` 里建一个以随机名命名的目标目录**
# （即使传了 `--destination`——那个参数在 `--test` 下被忽略，实测）。直接在模板根上跑，
# 等于每跑一次就往仓库里扔一个随机名字的目录。
#
# 模板仓库的写入口只能有一个，那是 `make lock`。所以这里复制到 `$GATE_ROOT` 底下再跑，
# 落下的随机目录连同整棵复制树一起删掉。复制用 tar 且排除 `.git` / `target`：那两样都不
# 参与展开，而 `target` 会让复制本身慢上几十秒。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,5p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "verify.sh: 不认识的参数：$1" ;;
    esac
done

# 门禁自己挑过的名字。随机名字撞上其中任何一个都说明 `--test` 的行为变了——它不该是
# 随机的了，而这一趟的价值有三分之一建立在"名字不是我挑的"上面。
GATE_PICKED_NAMES="demo-svc orders-gateway a-very-long-project-name-that-keeps-on-going-there x tokio relprobe migrate-svc"

gate_covers "cold-target"   "不复用编译缓存：CARGO_TARGET_DIR 未设置，展开树里没有 target"
gate_covers "expand-test"   "cargo-generate 的 --test 展开路径能走通"
gate_covers "random-name"   "项目名来自 cargo-generate 的随机词表，不是门禁挑的"
gate_covers "project-gate"  "在展开树里跑的是它自己的 make check，且是绿的"
gate_begin  "verify"

gate_workspace_init
gate_lock

W="$GATE_ROOT/verify"
safe_rm_rf "$W"
mkdir -p -- "$W/tpl"

# ── cold-target ─────────────────────────────────────────────────────────────
#
# 这一条得**在跑之前**断言。调用方 export 过 `CARGO_TARGET_DIR` 的话，下面那趟会安静地
# 复用缓存，而输出和真的冷编一模一样——「这一趟是冷的」会变成一句没人验过的话。
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    die "verify：环境里设了 CARGO_TARGET_DIR=$CARGO_TARGET_DIR。
   这一趟的价值之一就是从零编译——复用缓存时，一份用别的 feature 组合编出来的依赖可能
   让本该红的 feature 闭包变绿，而输出看不出区别。
   改法：env -u CARGO_TARGET_DIR make verify"
fi

( cd "$TEMPLATE_ROOT" && tar --exclude=./.git --exclude=./target -cf - . ) \
    | ( cd "$W/tpl" && tar -xf - ) \
    || die "verify：复制模板工作树失败"
[ ! -e "$W/tpl/target" ] || die "verify：复制出来的树里有 target/，排除规则没生效"
ok "cold-target：CARGO_TARGET_DIR 未设置，复制树里没有 target/"

# ── expand-test ─────────────────────────────────────────────────────────────
#
# `CARGO_GENERATE_TEST_CMD` **不过 shell**，它按空白切成 argv（实测：`pwd && ls` 会变成
# `pwd` 带两个参数）。所以这里只能是一个程序加参数——`make check`，正好就是 F5 说的
# 那个唯一入口。
log="$W/verify.log"
set +e
( cd "$W/tpl" && CARGO_GENERATE_TEST_CMD="make check" cargo generate --test ) \
    > "$log" 2>&1
status=$?
set -e

if [ "$status" -ne 0 ]; then
    tail -60 "$log" | sed 's/^/   /'
    die "verify：--test 这一趟红了（退出码 $status，完整日志 $log，上面是末 60 行）。
   注意 cargo-generate 把测试命令的失败也报成 \`Testing failed\`——先看上面的
   make check 输出，红的多半是生成结果自己的门禁，而不是展开本身。"
fi
ok "expand-test：cargo-generate --test 走通"

# ── random-name ─────────────────────────────────────────────────────────────
picked="$(sed -n 's/^.*project-name: \(.*\) \.\.\.$/\1/p' "$log" | head -1)"
[ -n "$picked" ] || {
    tail -20 "$log" | sed 's/^/   /'
    die "verify：从日志里读不出 project-name（见上）。
   cargo-generate 的输出格式变了，而 random-name 这条断言正是靠它——读不出来时
   不能默认它是随机的。"
}
for n in $GATE_PICKED_NAMES; do
    [ "$picked" != "$n" ] || die "verify：随机名字撞上了门禁自己挑的 \`$n\`。
   要么是天文数字的巧合，要么是 --test 不再随机取名了——后者会让这一趟退化成
   又一遍名字矩阵，而它本来是矩阵覆盖不到的那一片。"
done
info "本趟随机名字：$picked"
ok "random-name：名字由 cargo-generate 自己给出（$picked）"

# ── project-gate ────────────────────────────────────────────────────────────
#
# 退出码 0 只说明"没红"，不说明"跑的是 make check"。`CARGO_GENERATE_TEST_CMD` 一旦没生效，
# cargo-generate 会跑默认的 `cargo test`——那也会绿，而它漏掉了 fmt 与 clippy 两条。
#
# **不能拿日志里的 `Running "…"` 那行当判据**：cargo-generate 0.24.0 打的是写死的
# `Running "cargo test" ...`，哪怕它实际执行的是环境变量里那条命令（实测：命令换成 `pwd`，
# 日志照样写 `cargo test`，而跑的是 `pwd`）。照着那行断言，等于断言一个常量。
#
# 判据改成**只有 Makefile 才打得出来**的三行：两条子目标的分隔行，加收尾那条通过行。
# `cargo test` 一个都打不出来。
for marker in '── fmt ' '── lint ' '✓ check 通过'; do
    grep -qF -- "$marker" "$log" || {
        tail -30 "$log" | sed 's/^/   /'
        die "verify：日志里找不到 \"$marker\"——那是生成结果的 Makefile 才打得出来的。
   跑的多半是默认的 cargo test（漏掉 fmt 与 clippy），这一趟证不了 F5。
   注意别拿日志里的 \`Running \"…\"\` 行当证据，cargo-generate 那行是写死的。"
    }
done
ok "project-gate：展开树里跑的是 make check（fmt / lint / 通过行三条都在）"

safe_rm_rf "$W"
gate_end
