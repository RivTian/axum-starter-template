#!/usr/bin/env bash
# 门禁脚本的公共底座。所有 `scripts/*.sh` 第一句都 source 它。
#
# 这个文件只做三件事，每件都有一条具体的事故在后面顶着：
#
#   1. **破坏性操作的护栏**（`safe_rm_rf`）。门禁要反复重建生成目录，也就是要反复
#      `rm -rf`。一个算错的变量在这里的代价是删掉用户的工作树或者家目录。护栏是四层的，
#      逐层的理由写在函数头上。
#   2. **门禁入口的自述**（`gate_covers` / `gate_begin` / `gate_end`）。每个入口在开跑前
#      打印它实际覆盖的检查项清单。没有这一条，"门禁是绿的"和"门禁跑了什么"之间就只剩
#      信任——而清单一旦漏掉一项，绿色看起来完全一样。
#   3. **文本/二进制判别**（`is_text_file`）。全量文件扫描必须先判类型，否则一个
#      `.png` 里偶然出现的字节序列会把扫描打红，而那是不可复现的假红。
#
# 依赖面：bash 4+ 之外只用 POSIX 工具（`find` / `grep` / `sed` / `sort` / `perl`）。
# 不用 `flock`（macOS 没有）、不用 `realpath`（BSD 没有）、不用 `grep -P`（BSD 没有）。

# **可重复 source。** `scripts/tooling-test.sh` 要 source 别的门禁脚本去拿它们的纯函数，
# 而那些脚本各自第一句也 source 本文件。没有这道闸，第二次进来会撞上下面那一串 `readonly`
# ——bash 对"给只读变量赋值"的反应是报错并返回非零，在 `set -e` 下就是当场退出，而退出的
# 理由（"readonly variable"）与被测的东西毫无关系，排查起来要绕一大圈。
if [ -n "${GATE_LIB_SOURCED:-}" ]; then return 0; fi
GATE_LIB_SOURCED=1

set -euo pipefail

# ── 语言环境：钉死成 C ───────────────────────────────────────────────────────
#
# 门禁的判据不该随"谁的 shell"变。不钉的时候它会变，而且变的是两处**静默**的地方：
#
#   - `sort` / `comm` 的排序规则。多处相等比较靠的是两边用同一套排序（§3.3 邻接表、
#     未交付集合差）。`zh_CN.UTF-8` 下的排序规则与字节序不同，而 `comm` 只会对着一份
#     它认为没排好序的输入输出错误的差集——它不一定报错。
#   - 正则里**字符类**的含义。C 下 `[…]` 是字节集合，UTF-8 下是字符集合。一个写着
#     `[一二三四五六七八九]` 的字符类在 C 下要求"中间恰好一个字节"，于是它永远匹配不上
#     任何真实的中文——检查还在，分支已经死了，表现是永远绿。
#
# 选 C 而不是选某个 UTF-8：cargo 与 git 都按字节序排，C 与它们一致；而且当前全部门禁的
# 实测结果都是在 C 下取得的，钉 C 是保持行为，钉 UTF-8 是改行为。
#
# 代价随之而来的纪律：**正则里不许出现多字节字符类**，中文要枚举就写成 `(甲|乙)` 这种
# 分支。`scripts/tooling-test.sh::history-shape` 用 `第三阶段` 这条用例钉住了它——改回
# 字符类写法，那条会当场红。
export LC_ALL=C

# ── 仓库根：全脚本唯一的路径锚点 ──────────────────────────────────────────────
#
# 与生成结果里的那条纪律同源：一个进程只该有一个路径锚点。这里的锚点是**本文件所在目录
# 的父目录**，不是 `$PWD`——于是 `make` 从哪里被调用都不影响结果。
TEMPLATE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly TEMPLATE_ROOT

# 门禁的全部产物都落在这一个目录下。`safe_rm_rf` 只接受它底下的路径。
# macOS 的 `$TMPDIR` 带尾斜杠。必须先剥掉再拼：护栏里的祖先判断是纯字符串比较，
# `a//b` 与 `a/b` 在那里是两个不同的路径，而这个差别会让第 3 层护栏无缘无故地拒绝。
if [ -n "${GATE_ROOT:-}" ]; then
    GATE_ROOT="${GATE_ROOT%/}"
else
    _gate_tmp="${TMPDIR:-/tmp}"
    GATE_ROOT="${_gate_tmp%/}/axum-template-gate"
    unset _gate_tmp
fi
readonly GATE_ROOT

# 归属标记。`safe_rm_rf` 只删带这个标记的目录——见函数头第 3 层护栏。
readonly GATE_OWNER_MARK=".gate-owned-by-axum-starter-template"

# ── 输出 ────────────────────────────────────────────────────────────────────

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    readonly C_RED=$'\033[31m' C_GREEN=$'\033[32m' C_DIM=$'\033[2m' C_BOLD=$'\033[1m' C_OFF=$'\033[0m'
else
    readonly C_RED='' C_GREEN='' C_DIM='' C_BOLD='' C_OFF=''
fi

# 安静模式。名字矩阵那一类入口要把同一整套门禁跑四遍，逐项打印会把真正的进度埋掉。
#
# 开关放在**输出函数**里，不放在每个检查点上：检查点关心的是"过没过"，不关心谁在看。
# 反过来写（每处 `if [ "$QUIET" -eq 0 ]; then ok …; fi`）的代价是每加一项检查就多一处
# 可以忘记加的条件，而忘记的表现是"安静模式下多出一行"——没人会为此打红。
#
# `die` **不受它影响**：失败永远说话。安静的是进度，不是结论。
GATE_QUIET="${GATE_QUIET:-0}"

say()  { if [ "$GATE_QUIET" = 1 ]; then return 0; fi; printf '%s\n' "$*"; }
info() { if [ "$GATE_QUIET" = 1 ]; then return 0; fi; printf '   %s%s%s\n' "$C_DIM" "$*" "$C_OFF"; }
ok()   { if [ "$GATE_QUIET" = 1 ]; then return 0; fi; printf '   %s✓%s %s\n' "$C_GREEN" "$C_OFF" "$*"; }

# 失败一律走这里：同一种格式、同一个退出码、且写 stderr。
die() {
    printf '%s✗ %s%s\n' "$C_RED" "$*" "$C_OFF" >&2
    exit 1
}

# ── 门禁自述 ────────────────────────────────────────────────────────────────
#
# 用法：入口脚本在开跑前把每一项检查声明一遍，然后 `gate_begin` 打印清单。
#
#   gate_covers "unexpanded-placeholders" "生成树里不得残留 {{ … }}"
#   gate_begin  "gen"
#   …
#   gate_end
#
# 为什么要求先声明再执行：声明是**静态**的，跑了一半崩掉时清单已经打完了，读的人能看出
# 崩在哪一项之前。执行过程中边跑边打的话，崩溃点之后的项目根本不会出现在输出里——而那
# 正好是最需要知道"本该还要验什么"的时刻。
_GATE_COVER_IDS=()
_GATE_COVER_TEXTS=()
_GATE_NAME=""

gate_covers() {
    _GATE_COVER_IDS+=("$1")
    _GATE_COVER_TEXTS+=("$2")
}

gate_begin() {
    _GATE_NAME="$1"
    if [ "$GATE_QUIET" = 1 ]; then return 0; fi
    printf '\n%s▶ %s%s  （%d 项）\n' "$C_BOLD" "$_GATE_NAME" "$C_OFF" "${#_GATE_COVER_IDS[@]}"
    local i
    for i in "${!_GATE_COVER_IDS[@]}"; do
        printf '   %s·%s %-38s %s%s%s\n' \
            "$C_DIM" "$C_OFF" "${_GATE_COVER_IDS[$i]}" "$C_DIM" "${_GATE_COVER_TEXTS[$i]}" "$C_OFF"
    done
    printf '\n'
}

gate_end() {
    if [ "$GATE_QUIET" = 1 ]; then return 0; fi
    printf '\n%s✓ %s 通过%s（%d 项）\n' "$C_GREEN" "$_GATE_NAME" "$C_OFF" "${#_GATE_COVER_IDS[@]}"
}

# ── 破坏性操作护栏 ──────────────────────────────────────────────────────────

# 把路径规范化成绝对物理路径，**不跟随最后一段的符号链接**。
#
# 分两段做是有意的：`cd` 到父目录再 `pwd -P` 解掉父链上的所有链接，最后一段原样接回去。
# 一次性 `cd "$p" && pwd -P` 会把"这个路径本身是个链接"这件事悄悄抹平，而那正是护栏第 1
# 层要拦的东西。
abs_path() {
    local p="$1" parent base
    case "$p" in
        /*) ;;
         *) p="$PWD/$p" ;;
    esac
    parent="$(dirname "$p")"
    base="$(basename "$p")"
    [ -d "$parent" ] || die "abs_path: 父目录不存在：$parent"
    printf '%s/%s\n' "$(cd "$parent" && pwd -P)" "$base"
}

# 判断 `$1` 是不是 `$2` 的祖先（或就是它本身）。纯字符串比较，不碰文件系统。
path_is_ancestor_of() {
    local anc="${1%/}/" desc="${2%/}/"
    [ "${desc#"$anc"}" != "$desc" ]
}

# `$GATE_ROOT` 的物理路径。
#
# macOS 上 `/var` 是指向 `/private/var` 的符号链接，而 `$TMPDIR` 就在它下面。`abs_path`
# 会把目标解成 `/private/var/…`，而 `$GATE_ROOT` 还是 `/var/…`——拿这两个去比祖先关系
# 必然假阴，护栏会拒绝一个完全正当的删除。两边必须先落到同一个坐标系里。
#
# 目录还不存在时原样返回：此时也没有东西可删，后面那几层护栏一样拦得住。
gate_root_physical() {
    if [ -d "$GATE_ROOT" ]; then
        ( cd "$GATE_ROOT" && pwd -P )
    else
        printf '%s\n' "$GATE_ROOT"
    fi
}

# 带护栏的 `rm -rf`。
#
# 四层护栏，每一层拦的是不同的失误：
#
#   1. **路径本身不得是符号链接。** 拦的是"生成目录被人换成了指向别处的链接"。`rm -rf`
#      对一个指向目录的链接只会删掉链接本身，但这里的调用方随后会往这个路径写文件——
#      写进的就是链接的目标。与其事后惊讶，不如当场红。
#   2. **绝不是 `/`、`$HOME`、仓库根，也绝不是它们的祖先。** 拦的是变量算成空串或者算成
#      一段前缀。这一层单独存在的价值在于它**不依赖**第 3、4 层：哪怕前缀配置被人改错，
#      这几个路径也永远删不掉。
#   3. **必须在 `$GATE_ROOT` 之下。** 拦的是"删的是个合法目录，但不是我们的目录"。
#   4. **必须带归属标记文件。** 拦的是 `$GATE_ROOT` 指向了一个用户自己在用的目录——
#      比如有人把 `GATE_ROOT` 设成了 `~/work`。没有标记就说明这个目录不是门禁创建的，
#      门禁没有资格删它。
#
# 目标不存在时直接返回成功：门禁反复调用它，"本来就没有"和"删干净了"是同一个结果。
safe_rm_rf() {
    local target
    [ $# -eq 1 ] || die "safe_rm_rf: 需要且只需要一个参数"
    [ -n "$1" ] || die "safe_rm_rf: 目标为空——这几乎总是变量没展开"

    # 第 1 层。
    [ -L "$1" ] && die "safe_rm_rf: 目标是符号链接，拒绝：$1"

    # "本来就没有"和"删干净了"是同一个结果，直接返回。这一步必须在 `abs_path` **之前**：
    # 门禁反复调用它，而第一次调用时连父目录都还不存在，`abs_path` 会在那里报一个与本意
    # 无关的错。
    [ -e "$1" ] || return 0

    target="$(abs_path "$1")"

    # 第 2 层。`$HOME` 可能没设，那就不参与比较，但 `/` 和仓库根永远参与。
    local forbidden=("/" "$TEMPLATE_ROOT")
    [ -n "${HOME:-}" ] && forbidden+=("$HOME")
    local f
    for f in "${forbidden[@]}"; do
        [ "$target" = "${f%/}" ] && die "safe_rm_rf: 拒绝删除受保护路径：$target"
        path_is_ancestor_of "$target" "$f" && die "safe_rm_rf: 目标是受保护路径的祖先：$target ⊃ $f"
    done

    # 第 3 层。
    local gate_phys
    gate_phys="$(gate_root_physical)"
    path_is_ancestor_of "$gate_phys" "$target" \
        || die "safe_rm_rf: 目标不在门禁工作区内：$target（工作区 $gate_phys）"

    # 第 4 层。标记要么在目标自己身上，要么在 `$GATE_ROOT` 上——后者覆盖"删的是工作区里
    # 的一个子目录"这种常规情况。
    [ -e "$target/$GATE_OWNER_MARK" ] || [ -e "$GATE_ROOT/$GATE_OWNER_MARK" ] \
        || die "safe_rm_rf: 缺少归属标记，拒绝：$target"

    rm -rf -- "$target"
}

# 创建（或确认）门禁工作区，并打上归属标记。
gate_workspace_init() {
    mkdir -p -- "$GATE_ROOT"
    [ -L "$GATE_ROOT" ] && die "gate_workspace_init: 工作区是符号链接，拒绝：$GATE_ROOT"
    : > "$GATE_ROOT/$GATE_OWNER_MARK"
}

# ── 互斥 ────────────────────────────────────────────────────────────────────
#
# 门禁的多个目标共享 `$GATE_ROOT`，并行跑（`make -j`）会互相删对方的树。
#
# 用 `mkdir` 而不是 `flock`：`flock(1)` 在 macOS 上根本不存在，而 `mkdir` 的原子性是
# POSIX 保证的、到处都有。代价是崩溃后锁不会自动释放，所以锁目录里记 PID，陈旧锁可辨认。
#
# **可重入**，靠 `GATE_LOCK_HELD` 沿进程树往下传。`matrix.sh` 这样的复合入口会连着调用
# `gen.sh` 和 `structure.sh`，而那两个各自也要锁——不传的话，父进程持锁、子进程等锁，
# 等满 600 秒然后红，而红的原因与被测的东西毫无关系。持锁者的子进程本来就在同一段临界区
# 里，它们不是竞争者。
gate_lock() {
    if [ "${GATE_LOCK_HELD:-0}" = 1 ]; then return 0; fi
    local lock="$GATE_ROOT/.lock" waited=0
    mkdir -p -- "$GATE_ROOT"
    while ! mkdir -- "$lock" 2>/dev/null; do
        if [ -f "$lock/pid" ] && ! kill -0 "$(cat "$lock/pid")" 2>/dev/null; then
            info "清理陈旧锁（持有者 $(cat "$lock/pid") 已不在）"
            rm -rf -- "$lock"
            continue
        fi
        waited=$((waited + 1))
        [ "$waited" -gt 600 ] && die "gate_lock: 等锁超过 600 秒：$lock"
        sleep 1
    done
    printf '%s\n' "$$" > "$lock/pid"
    export GATE_LOCK_HELD=1
    # shellcheck disable=SC2064  # 这里要的正是**当下**的 $lock 值，不是退出时再求值。
    trap "rm -rf -- '$lock'" EXIT
}

# ── 文本/二进制判别 ─────────────────────────────────────────────────────────
#
# 全量文件扫描（未展开占位符、历史标记、语言纪律）必须先过这一关：一个 `.png` 或
# `.sqlite` 里偶然出现的字节序列会让扫描报出一条谁都看不懂的假红，而且是不可复现的假红
# ——换一张图就没了。
#
# 判据是"前 8 KiB 里有没有 NUL 字节"，与 git 的判据一致。不看扩展名：扩展名是约定，
# 而这里要的是事实。
#
# 判别用 perl 而不是 `grep -q $'\000'`：BSD grep 处理不了模式里的 NUL，而这个脚本要在
# macOS 上跑。perl 的 `index` 对二进制串没有任何歧义。
is_text_file() {
    [ -f "$1" ] || return 1
    perl -e 'open(my $fh, "<", $ARGV[0]) or exit 1; binmode $fh;
             read($fh, my $b, 8192); exit(index($b, "\0") >= 0 ? 1 : 0)' -- "$1"
}

# 列出一棵树里的全部文本文件（NUL 分隔，供 `while IFS= read -r -d ''` 消费）。
#
# 跳过 `.git` 与 `target`：前者是元数据不是内容，后者是构建产物，两者都会让扫描慢上两个
# 数量级而且只会扫出噪声。
#
# 判别整棵树只起**一个** perl 进程。逐文件起一个也能对，但那是几百次 fork，而门禁要在
# 名字矩阵里把整套扫描跑四遍。
list_text_files() {
    find "$1" \( -name .git -o -name target \) -prune -o -type f -print0 \
        | perl -0 -ne 'chomp; open(my $fh, "<", $_) or next; binmode $fh;
                       read($fh, my $b, 8192);
                       print "$_\0" unless index($b, "\0") >= 0'
}
