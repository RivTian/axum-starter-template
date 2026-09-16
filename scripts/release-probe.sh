#!/usr/bin/env bash
# DP2 的产物级证据：release 构建下，任务 panic 仍然可分类、进程仍然活着。
#
#   scripts/release-probe.sh [--quiet]
#
# ## 为什么不是读一眼 manifest 就完事
#
# DP2 选了 `panic = "unwind"`，理由是三条纪律都挂在它上面：`ExitKind::Panicked` 这个分类要
# 真的能取到、`CatchPanicLayer` 要真的能兜住一个 handler 的 panic、release 下的不变量检查
# 要真的还看得见。
#
# 断言这件事最省事的写法是 `grep 'panic = "unwind"' Cargo.toml`。那是一条**关于文件内容**
# 的断言，而要证的是一条**关于产物行为**的结论，中间隔着 cargo 怎么解释 profile、
# `RUSTFLAGS` 有没有覆盖它、`--target` 在不在场这一整条链。链上任何一环变了，grep 照样绿。
#
# 所以这里真的构建一个 release 产物、真的跑它、逐行比对它的 stdout。产物自己报告
# `debug_assertions=false`——「这是不是真的 release」这个问题因此由产物回答，不由脚本声称。
#
# ## 为什么带 `--target <host-triple>`
#
# 不带 `--target` 时，cargo 用同一份 profile 去编译**宿主工具**（build script、proc macro）。
# 负向探针要把 panic 策略改成 `abort`，而 proc macro 必须以 unwind 编译（编译器自己要在
# 展开失败时收住），于是不带 `--target` 的负向探针会红在一个和被测纪律无关的地方，报错也
# 完全不提 `compile_error!`。带上 `--target` 之后宿主工具走自己的那一套，被测的只剩目标
# 产物——这条区分是这个探针能给出可读失败的前提。
#
# 正向那一趟也带 `--target`，为的是两趟落在同一棵 target 子树下，第三方依赖只编一次。
#
# ## 负向探针为什么改 profile，而不是设 `RUSTFLAGS`
#
# 两条路都能把 `cfg(panic = "abort")` 打开，`core/src/task/exit.rs` 的 `compile_error!` 两
# 边都会红。选改 profile 是因为它是**真的会发生**的那一种：有人为了"减小体积"顺手把
# `panic = "abort"` 写进 `[profile.release]`，这件事在真实仓库里每年都发生几次。而
# `RUSTFLAGS="-C panic=abort"` 那条路验的是 cargo 怎么把 flag 转成 cfg——那是 cargo 的行为，
# 不是本模板的纪律，再编一遍全部依赖去证它不划算。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HERE="$(dirname "${BASH_SOURCE[0]}")"

# 专用的一次性名字：这条门禁会往生成树里塞一个 example、还会改根清单的 profile。
REL_NAME="relprobe"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "不认识的参数：$1" ;;
    esac
done

gate_covers "release-artifact" "真的是 release 产物——由产物自己报告 debug_assertions=false"
gate_covers "panic-classified" "任务 panic 被分类成 panicked 且消息取得到（逐行精确比对）"
gate_covers "process-survives" "一个任务 panic 不会杀掉进程：退出码 0"
gate_covers "abort-guard"      "负向：profile 改成 panic=abort 时编译期守卫必须红"
gate_begin  "release-probe"

gate_workspace_init
gate_lock

DEST="$GATE_ROOT/gen/$REL_NAME"
WORK="$GATE_ROOT/release-probe"
mkdir -p -- "$WORK"

# 宿主三元组由 rustc 自报。写死一个（比如 `aarch64-apple-darwin`）就等于把这条门禁钉在
# 一台机器上，而它本该在任何能跑 Rust 的地方都成立。
HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$HOST_TRIPLE" ] || die "release-probe：从 rustc -vV 里读不出 host 三元组"

bash "$HERE/gen.sh" --name "$REL_NAME" --quiet

PREFIX="$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\(.*\)-core"[[:space:]]*$/\1/p' \
    "$DEST/core/Cargo.toml" | head -1)"
[ -n "$PREFIX" ] || die "release-probe：从 $DEST/core/Cargo.toml 里读不出包名前缀"

export CARGO_TARGET_DIR="$GATE_ROOT/cache"
BIN="$CARGO_TARGET_DIR/$HOST_TRIPLE/release/examples/release_panic"

# ── 探针程序 ────────────────────────────────────────────────────────────────
#
# 写进生成树而不是放在 `scripts/fixtures/` 下：它是这条门禁的一部分，不是模板的一部分。
# 放成一个独立文件的话，`structure.sh::absent` 要多一条"它不许进生成结果"的名单项，而模板
# 树里会多一个每次有人看见都要先判断"这算交付物吗"的 `.rs`。
#
# 程序与它的期望输出隔着二十行写在同一个文件里——这是能做到的最紧的耦合。改了打印就必须
# 改下面那段期望，反过来也一样，没有第三个地方需要同步。
#
# 用 `new_current_thread`：要证的是 panic 怎么被分类，不是调度器有几个线程。单线程还顺带
# 让"这个 example 只靠 core 的 normal 依赖就能跑"成立（`rt-multi-thread` 只有 app 能开）。
mkdir -p -- "$DEST/core/examples"
cat > "$DEST/core/examples/release_panic.rs" <<'RUST'
//! 门禁探针：由 scripts/release-probe.sh 写入，同一次运行里随生成树一起删掉。
//!
//! 它回答一个只有真实产物能回答的问题：release 下一个顶层任务 panic 之后，
//! 监督者还能不能把它分类成 `Panicked`、进程还活不活着。

// `unused_crate_dependencies` 是按**目标**算的：这个 example 只 use 了 `service_core` 与
// `tokio`，于是 core 的另外四条依赖（serde / humantime-serde / thiserror / tracing）在这里
// 各报一条"未使用"。那四条是给 lib 目标用的，与探针无关。
//
// 不放任它们打出来，是因为构建失败时这份日志会被整段贴给人看——四条与被测纪律无关的
// 警告会把真正的错误挤到看不见的地方。
#![allow(unused_crate_dependencies)]

use service_core::task::{ExitKind, ShutdownClass, TaskName, TaskSpec, TaskSupervisor};

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread runtime");

    let mut supervisor = TaskSupervisor::new();
    supervisor
        .spawn_on(
            TaskSpec::new(TaskName::Ticker, ShutdownClass::Graceful),
            rt.handle(),
            Box::pin(async { panic!("release probe: deliberate panic") }),
        )
        .expect("register the probe task");

    let exit = rt
        .block_on(supervisor.next_exit())
        .expect("the supervisor had exactly one task");

    // 每行一个事实，顺序固定。门禁逐行比对，多一行少一行都红。
    println!("name={}", exit.name_str());
    println!("kind={}", exit.kind.as_str());
    println!(
        "message={}",
        match &exit.kind {
            ExitKind::Panicked(summary) => summary.to_string(),
            other => format!("<not a panic: {}>", other.as_str()),
        }
    );
    println!("terminal={}", exit.kind.is_terminal());
    println!("failure={}", exit.kind.is_failure());
    // 「这是不是真的 release」由产物自己回答。脚本说了不算——脚本正是可能传错参数的那一方。
    println!("debug_assertions={}", cfg!(debug_assertions));
}
RUST

# ── 正向：构建并运行 ────────────────────────────────────────────────────────
( cd "$DEST" && cargo build --release --target "$HOST_TRIPLE" \
        --example release_panic -p "$PREFIX-core" --locked ) > "$WORK/build.log" 2>&1 \
    || { sed 's/^/   /' "$WORK/build.log"; die "release-probe：正向构建失败（见上）。"; }

[ -x "$BIN" ] || die "release-probe：构建说成功了，但产物不在预期位置：
   $BIN
   （cargo 换了 example 的输出布局，或者 --target 没有生效）"

set +e
"$BIN" > "$WORK/stdout.txt" 2> "$WORK/stderr.txt"
PROBE_STATUS=$?
set -e

# ── panic-classified：逐行精确比对 ──────────────────────────────────────────
#
# 相等比较，不是"含有"。含有放得过多打印一行——而多出来的那一行可能正是某个分类走错了
# 之后新长出来的。
cat > "$WORK/expected.txt" <<'EXPECTED'
name=ticker
kind=panicked
message=release probe: deliberate panic
terminal=true
failure=true
debug_assertions=false
EXPECTED

if ! diff -u "$WORK/expected.txt" "$WORK/stdout.txt" > "$WORK/stdout.diff"; then
    sed 's/^/   /' "$WORK/stdout.diff"
    die "release-probe：产物的输出与期望逐行比对不符（见上，左为期望）。
   若 debug_assertions 那行是 true：编出来的根本不是 release，前面几行的结论全部作废。
   若 kind 那行不是 panicked：分类在 release 下走了别的分支——这正是 DP2 选 unwind 要防的事。
   若 message 那行是空的：panic 负载没取到，PanicSummary::from_payload 的两种形态覆盖不全了。"
fi
ok "release-artifact：debug_assertions=false，由产物自报"
ok "panic-classified：六行逐行相等"

# ── process-survives ────────────────────────────────────────────────────────
#
# 这是 DP2 最核心的那条产物级事实：任务 panic 了，进程没有。`abort` 之下这里会是
# 一个信号终止（134/SIGABRT），而且前面那六行根本打不出来。
if [ "$PROBE_STATUS" -ne 0 ]; then
    sed 's/^/   /' "$WORK/stderr.txt"
    die "process-survives：探针进程的退出码是 $PROBE_STATUS，期望 0。
   一个顶层任务 panic 之后整个进程跟着死，说明展开没有在 JoinSet 边界上被接住——
   panic 策略实际不是 unwind（134 通常就是 SIGABRT）。"
fi

# 默认 panic hook 应当在 stderr 上留下痕迹。它不进上面那份逐行比对（hook 的措辞是 std 的，
# 会随版本变），但"一条都没有"说明 hook 被谁换掉了，那是另一类需要知道的事。
grep -q 'release probe: deliberate panic' "$WORK/stderr.txt" \
    || die "process-survives：stderr 里找不到 panic 的痕迹。
   默认 panic hook 被替换了？分类拿到了消息、而人读的那一份却没有，
   是一种只在事故当天才会发现的静默。"
ok "process-survives：退出码 0，stderr 留下了 panic 记录"

# ── abort-guard：负向 ───────────────────────────────────────────────────────
#
# 改根清单的 profile，再编一次。期望**编译失败**，而且失败的必须是那条 `compile_error!`
# ——不是随便哪个错误都算数。
sed -i '' 's/^panic = "unwind"$/panic = "abort"/' "$DEST/Cargo.toml"
grep -q '^panic = "abort"$' "$DEST/Cargo.toml" \
    || die "abort-guard：没能把 $DEST/Cargo.toml 的 [profile.release] 改成 abort。
   根清单里那一行的写法变了（期望正好是 panic = \"unwind\" 独占一行）。
   这条负向探针没有跑起来——不要当成它绿了。"

set +e
( cd "$DEST" && cargo build --release --target "$HOST_TRIPLE" \
        --example release_panic -p "$PREFIX-core" --locked ) > "$WORK/abort.log" 2>&1
ABORT_STATUS=$?
set -e

if [ "$ABORT_STATUS" -eq 0 ]; then
    die "abort-guard：把 panic 改成 abort 之后，构建**竟然成功了**。
   core/src/task/exit.rs 里的 cfg(panic = \"abort\") 守卫没有生效。
   后果：ExitKind::Panicked 在 release 下永远取不到，而四分类看上去还是四个变体——
   退出分类会静默地从四选一变成三选一，没有任何一处会报错。"
fi
grep -q 'this workspace requires' "$WORK/abort.log" || {
    sed 's/^/   /' "$WORK/abort.log"
    die "abort-guard：构建确实失败了，但失败的不是那条编译期守卫（见上）。
   一个碰巧失败的构建不能当成守卫生效的证据——它明天可能因为别的原因失败，
   而那时这条断言仍然绿着。"
}
ok "abort-guard：panic=abort 时编译期守卫如期拦住"

safe_rm_rf "$DEST"
gate_end
