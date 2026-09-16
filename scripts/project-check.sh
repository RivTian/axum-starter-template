#!/usr/bin/env bash
# 在生成出来的工程里跑**它自己的** `make check`，并验它确实跑全了。
#
#   scripts/project-check.sh [--name NAME] [--quiet]
#
# ## 为什么"绿了"不等于"跑了"
#
# F5 说生成结果的 `make check` 是门禁的唯一定义。那句话要成立，得先有人证明 `check` 真的
# 是那三条的并集——而 `make check` 退出码 0 恰恰**证明不了**这件事：
#
#   · `check: fmt lint test` 里掉了一条依赖，照样退出 0。
#   · `test` 里 D66 那趟 `--features …/test-utils` 被删掉，照样退出 0（第一趟还在）。
#   · doctest 计数那段 `if` 被改成永真的形态，照样退出 0。
#
# 三种改动的共同点：**输出变短了，退出码没变**。所以这条门禁读日志，而且读的是那些
# "只有真跑过才可能出现"的行——Makefile 自己打的分隔行，和一个只有 feature 打开才编译
# 得出来的测试名。
#
# ## 编译缓存是共用的，这是故意的
#
# `CARGO_TARGET_DIR` 指向 `$GATE_ROOT/cache`，与名字矩阵那四趟共用。不共用的话这一条门禁
# 要编十几分钟，而一个要等十几分钟的门禁没人会在提交前跑。
#
# 共用的代价是"第一次构建能不能成"在这里**没有**被验——缓存里可能躺着一份用别的 feature
# 组合编出来的依赖。那件事是 `make verify` 的职责（它不设 `CARGO_TARGET_DIR`，从零编），
# 写在这里是为了说明这个代价有人接着，不是被忽略了。
#
# ## 它不自己生成
#
# 需要生成树已经在那儿。`check` 里 `gen` 排在它前面，两条看的是**同一棵**树——自己再生成
# 一次的话，`structure` 验过的树和这里跑 `make check` 的树就成了两棵，而它们之间没有任何
# 东西保证一致。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

GEN_NAME="demo-svc"

while [ $# -gt 0 ]; do
    case "$1" in
        --name)  GEN_NAME="${2:?--name 后面要跟名字}"; shift 2 ;;
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "project-check.sh: 不认识的参数：$1" ;;
    esac
done

gate_covers "default-goal"   "裸 make 打的是 help，不是闷头启动一趟几分钟的构建"
gate_covers "project-gate"   "make check 绿，且 fmt / lint / test 三条都真的执行了"
gate_covers "storage-feature" "D66 的第二趟 --features …/test-utils 真的跑了"
gate_covers "doctest-zero"   "doctest 计数那段检查执行过，且判为 0"
gate_covers "cache-shared"   "编译产物落在共用缓存里，生成树自己不留 target/"
gate_begin  "project-check"

gate_workspace_init
gate_lock

DEST="$GATE_ROOT/gen/$GEN_NAME"
[ -d "$DEST" ] || die "project-check：生成树 $DEST 不在。
   这条门禁不自己生成——它要和 structure 看同一棵树。
   先跑：scripts/gen.sh --name $GEN_NAME"

# ── default-goal ────────────────────────────────────────────────────────────
#
# 用 `-n`（只打印不执行）：DEFAULT_GOAL 万一真的掉成了 `check`，这里不该因为验一条纪律
# 而顺手启动三分钟的构建。`-n` 会把 `@` 前缀的命令也打出来，于是两种情形在输出上区分得开。
dry="$GATE_ROOT/project-check-dry.log"
make -C "$DEST" -n > "$dry" 2>&1 \
    || { tail -20 "$dry" | sed 's/^/   /'; die "project-check：裸 make -n 就失败了，日志见 $dry"; }

grep -qF -- 'make check' "$dry" \
    || { tail -20 "$dry" | sed 's/^/   /'; die "project-check：裸 make 没打出 help（见上）。"; }
# 写成 `if` 而不是 `grep … && die`：后者在 grep 无匹配（也就是**正常**那条路）时整个
# AND 列表的退出码是 1，于是这行代码的正确与否取决于 `set -e` 对 AND 列表的细则，
# 以及它在脚本里的位置。换个位置就可能变成"一切正常时脚本退出"。
if grep -qF -- 'cargo fmt' "$dry"; then
    die "project-check：裸 make 会去跑 cargo fmt。
   .DEFAULT_GOAL 掉了，或者 help 不再是第一个目标。
   后果：第一次 clone 下来的人敲一个 make，等来的是几分钟的全量构建而不是一页说明。"
fi
ok "default-goal：裸 make 打 help，不触发构建"

# ── project-gate ────────────────────────────────────────────────────────────
log="$GATE_ROOT/project-check.log"
set +e
( cd "$DEST" && CARGO_TARGET_DIR="$GATE_ROOT/cache" make check ) > "$log" 2>&1
status=$?
set -e

if [ "$status" -ne 0 ]; then
    tail -60 "$log" | sed 's/^/   /'
    die "project-check：生成结果自己的 make check 红了（退出码 $status，完整日志 $log，上面是末 60 行）。"
fi

for marker in '── fmt ' '── lint ' '── test ' '✓ check 通过'; do
    grep -qF -- "$marker" "$log" \
        || die "project-check：日志里找不到 \"$marker\"。
   check 退出 0 但少打了这一行，说明 \`check: fmt lint test\` 的依赖掉了一条。
   退出码看不出区别——这正是这条断言存在的理由。日志：$log"
done
ok "project-gate：make check 绿，fmt / lint / test 三条分隔行与通过行都在"

# ── storage-feature ─────────────────────────────────────────────────────────
#
# 判据是一个**只有 `test-utils` 打开才编译得出来**的测试名。拿 Makefile 里那行
# `· 存储门面的可替换性：…` 当判据是不行的——那是一句 echo，删掉 cargo 调用它照样打。
FEATURE_ONLY_TEST="debug_shows_the_backend_name_only"
grep -qF -- "$FEATURE_ONLY_TEST" "$log" \
    || die "project-check：日志里没有 $FEATURE_ONLY_TEST。
   那个测试住在 storage 的 memory 模块里，整个模块是 #[cfg(feature = \"test-utils\")]，
   所以它出现 = 第二趟带 feature 的 cargo test 真的跑了，不出现 = 那趟没了。
   没了的后果（D66）：InMemoryStorage 在默认 feature 下永远不编译，静默腐烂，
   而门面「可替换」这件事就此失去全部证据。日志：$log"
ok "storage-feature：D66 的第二趟跑了（$FEATURE_ONLY_TEST 在日志里）"

# ── doctest-zero ────────────────────────────────────────────────────────────
grep -qF -- 'doctest 计数 0' "$log" \
    || die "project-check：日志里没有 doctest 计数的收尾行。
   那段检查要么被删了，要么被改成了不会打这行的形态。它退出 0 与它根本没跑，
   在退出码上一模一样。日志：$log"
ok "doctest-zero：计数检查执行过，判为 0"

# ── cache-shared ────────────────────────────────────────────────────────────
#
# 在 `make check` **之后**断言：跑之前生成树里本来就没有 target/，那时断言等于什么都没验。
[ -d "$GATE_ROOT/cache" ] || die "project-check：共用缓存 $GATE_ROOT/cache 没建起来，
   CARGO_TARGET_DIR 没被 cargo 认。"
[ ! -e "$DEST/target" ] || die "project-check：生成树里出现了 target/。
   CARGO_TARGET_DIR 漏掉了某一条 cargo 调用（子 make？脚本里另起的 cargo？）。
   后果：名字矩阵那四趟各编一份，门禁从几分钟变成几十分钟。"
ok "cache-shared：产物在 $GATE_ROOT/cache，生成树里没有 target/"

gate_end
