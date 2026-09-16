#!/usr/bin/env bash
# 树外生成一个工程，并**当场审计**生成出来的那棵树。
#
# 用法：
#   scripts/gen.sh [--name NAME] [--git] [--quiet]
#
# 生成目录固定在 `$GATE_ROOT/gen/<NAME>`，调用方不需要接收路径——它是算得出来的。
#
# ## 为什么审计写在这里，而不是单独一个目标
#
# 审计的对象是**生成出来的那棵树**，不是模板树。两者有一半的文件不同（渲染过的、被
# `ignore` 删掉的、被后置钩子改名的），所以在模板树上扫是扫不到真相的。把生成和审计绑在
# 一个脚本里，"生成成功了但没人验"这个状态就不存在。
#
# ## 为什么先删再生成
#
# cargo-generate 对已存在的目录是覆盖写，不是替换。不清理的话，模板里被删掉的文件会永远
# 留在生成目录里继续被后面的结构检查当成"存在"——一个已经删掉的文件可以让门禁绿上好几
# 个月。这里每次都把源码树整个删掉重来。
#
# 构建产物不受影响：生成树里根本没有 `target/`，编译缓存由调用方用 `CARGO_TARGET_DIR`
# 挂到别处。清理源码树和保留编译缓存因此不冲突。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# 五项扫描的判据（正则 / 豁免清单 / 集合差）都在这里，理由也写在那边。
# 它们单独成文件，是为了让 `scripts/tooling-test.sh` 能拿合成输入证明每一条真的能红——
# 内联在下面的主流程里时，那件事只能靠读正则来回答。
# shellcheck source=scripts/audit-rules.sh
. "$(dirname "${BASH_SOURCE[0]}")/audit-rules.sh"

GEN_NAME="demo-svc"
GEN_VIA="path"
while [ $# -gt 0 ]; do
    case "$1" in
        --name)  GEN_NAME="$2"; shift 2 ;;
        --git)   GEN_VIA="git"; shift ;;
        --quiet) GATE_QUIET=1; shift ;;
        *)       die "gen.sh: 不认识的参数：$1" ;;
    esac
done

GEN_DEST="$GATE_ROOT/gen/$GEN_NAME"

gate_covers "fresh-tree"             "生成前先删掉整棵源码树，不做增量覆盖"
gate_covers "unexpanded-placeholder" "生成树里不得残留 {{ … }}（豁免逐条带理由）"
gate_covers "history-marker"         "不得残留模板自身的里程碑 / 闸门 / 阶段编号"
gate_covers "undelivered-reference"  "不得引用任何没随交付发布的文件（清单动态算出）"
gate_covers "template-trace"         "随交付文件不得出现设计稿节号、纪律编号、仓库自身的名字"
gate_covers "single-gate-entry"      "README 与 Makefile 不得出现「另外还要跑」式的第二入口"
gate_begin "gen（$GEN_NAME，经 --$GEN_VIA）"

gate_workspace_init
gate_lock

# ── 生成 ────────────────────────────────────────────────────────────────────

mkdir -p -- "$GATE_ROOT/gen"
safe_rm_rf "$GEN_DEST"
gen_log="$GATE_ROOT/gen-$GEN_NAME.log"

if [ "$GEN_VIA" = "git" ]; then
    # 走用户真实的 `--git` 安装路径。
    #
    # 这一趟的价值不在"再生成一次"，在于它是**另一条代码路径**：cargo-generate 对 git 源
    # 会先 clone 出一个新的 checkout，于是两件在 `--path` 那条路上被完全绕过的事第一次
    # 参与进来——`.gitattributes` 的换行符规则，以及"哪些文件真的被提交了"。后者尤其重要：
    # 一个忘了 `git add` 的文件在 `--path` 下永远是绿的，在用户那里永远是缺的。
    #
    # 源仓库是**当场从工作树快照出来的**，不是克隆模板仓库自己的 `.git`。这条门禁要验的是
    # "我现在写的这棵树经 git 发出去会是什么样"，而不是"上一次提交是什么样"；顺带地，它
    # 在一个还没建立提交历史的模板上也能跑。
    src="$GATE_ROOT/gitsrc"
    safe_rm_rf "$src"
    mkdir -p -- "$src"
    ( cd "$TEMPLATE_ROOT" && tar --exclude=./.git --exclude=./target -cf - . ) \
        | ( cd "$src" && tar -xf - ) \
        || die "快照模板工作树失败"
    (
        cd "$src" \
            && git init -q \
            && git add -A \
            && git -c user.email=gate@invalid -c user.name=gate \
                   commit -q -m "snapshot for --git gate"
    ) >"$gen_log" 2>&1 || { cat "$gen_log" >&2; die "快照仓库初始化失败，日志见 $gen_log"; }

    ( cd "$GATE_ROOT/gen" && cargo generate --git "$src" --name "$GEN_NAME" --force ) \
        >>"$gen_log" 2>&1 \
        || { cat "$gen_log" >&2; die "cargo generate --git 失败，日志见 $gen_log"; }
else
    ( cd "$GATE_ROOT/gen" && cargo generate --path "$TEMPLATE_ROOT" --name "$GEN_NAME" --force ) \
        >"$gen_log" 2>&1 \
        || { cat "$gen_log" >&2; die "cargo generate --path 失败，日志见 $gen_log"; }
fi

[ -d "$GEN_DEST" ] || die "生成完成了，但 $GEN_DEST 不存在——cargo-generate 把它放到别处去了？"
ok "fresh-tree：$GEN_DEST"

# ── 扫描的公共底座 ──────────────────────────────────────────────────────────
#
# 后面每一项扫描都在同一份文件清单上跑，只算一次。
#
# 清单只含**文本**文件：一个 `.sqlite3` 或 `.png` 里偶然出现的字节序列会让扫描报出一条
# 不可复现的假红——换一份数据就没了。判据是"前 8 KiB 有没有 NUL"，与 git 一致。
text_list="$GATE_ROOT/gen-$GEN_NAME.textfiles"
list_text_files "$GEN_DEST" > "$text_list"
text_count="$(perl -0 -ne 'END { print 0 + $. }' < "$text_list")"
[ "${text_count:-0}" -gt 0 ] || die "生成树里一个文本文件都没有——扫描的前提不成立"

# `-H` 不能省：xargs 会按参数长度分批，最后一批**可能只剩一个文件**，而 grep 在只有一个
# 文件时默认不打文件名。于是同一次扫描的输出里有的行带路径、有的不带——不带的那几行恰恰
# 最难定位。这类"只在文件数刚好是某个值时才出现"的输出差异不该留给下一个人去发现。
scan_re()    { xargs -0 grep -nHE -- "$1" < "$text_list" 2>/dev/null || true; }
scan_fixed() { xargs -0 grep -nHF -- "$1" < "$text_list" 2>/dev/null || true; }

# 相对路径，输出里不带 `$GATE_ROOT` 那一长串。
rel() { printf '%s\n' "${1#"$GEN_DEST"/}"; }

# 报错统一走这里：先打命中行，再说清楚这一项为什么存在、该怎么改。
report_and_die() {
    printf '%s✗ %s%s\n' "$C_RED" "$1" "$C_OFF" >&2
    printf '%s\n' "$2" | sed "s|^$GEN_DEST/|   |" >&2
    printf '\n   %s\n' "$3" >&2
    exit 1
}

# ── 1. 未展开占位符 ─────────────────────────────────────────────────────────
#
# 判据与豁免清单见 `scripts/audit-rules.sh`（B-22）。
hits=""
while IFS= read -r line; do
    [ -n "$line" ] || continue
    if ! audit_placeholder_exempt "$(rel "${line%%:*}")"; then
        hits="$hits$line"$'\n'
    fi
done < <(scan_re "$AUDIT_PLACEHOLDER_RE")
if [ -n "$hits" ]; then
    report_and_die "unexpanded-placeholder" "$hits" \
        "生成结果里还有没展开的模板占位符。两种可能：这个文件该进 include 白名单，
   或者这里的 \`{{\` 是 Rust 的格式串转义、该进脚本里那份豁免清单——加豁免时
   必须同时写清楚它为什么不是泄漏。"
fi
ok "unexpanded-placeholder（豁免 4 个 .rs，均为格式串转义）"

# ── 2. 历史标记 ─────────────────────────────────────────────────────────────
#
# 判据与它两处收窄的理由见 `scripts/audit-rules.sh`（B-24）。
hits="$(scan_re "$AUDIT_HISTORY_RE")"
if [ -n "$hits" ]; then
    report_and_die "history-marker" "$hits" \
        "生成结果里带着模板自己的开发时间表。拿到这个项目的人没有那张表，这些编号对他
   只是噪声。改法是把它替换成它**指代的事实本身**，而不是删掉了事。"
fi
ok "history-marker"

# ── 3. 未交付文件的引用 ─────────────────────────────────────────────────────
#
# 清单是**算出来**的，不是写死的；集合差与基名撞车的处理见 `scripts/audit-rules.sh`（B-21）。
undelivered="$GATE_ROOT/gen-$GEN_NAME.undelivered"
tpl_files="$GATE_ROOT/gen-$GEN_NAME.tpl-files"
gen_files="$GATE_ROOT/gen-$GEN_NAME.gen-files"
gen_names="$GATE_ROOT/gen-$GEN_NAME.gen-basenames"
( cd "$TEMPLATE_ROOT" && find . \( -name .git -o -name target \) -prune -o -type f -print ) \
    | sed 's|^\./||' | sort > "$tpl_files"
( cd "$GEN_DEST" && find . \( -name .git -o -name target \) -prune -o -type f -print ) \
    | sed 's|^\./||' | sort > "$gen_files"
sed 's|.*/||' "$gen_files" | sort -u > "$gen_names"
audit_undelivered_paths "$tpl_files" "$gen_files" "$gen_names" > "$undelivered"

hits=""
while IFS= read -r path; do
    [ -n "$path" ] || continue
    audit_undelivered_exempt "$path" && continue
    found="$(scan_fixed "$path")"
    if [ -n "$found" ]; then hits="$hits$found"$'\n'; fi
done < "$undelivered"
if [ -n "$hits" ]; then
    report_and_die "undelivered-reference" "$hits" \
        "生成结果引用了一个它自己没有的文件。读到这行的人会去找，然后找不到。
   这份未交付清单是从「模板树有、生成树没有」算出来的，不是写死的——所以它不会误报
   一个其实发布了的文件，这条红是真的。"
fi
ok "undelivered-reference（未交付路径 $(grep -c . "$undelivered" || true) 条）"

# ── 4. 模板自身的痕迹 ───────────────────────────────────────────────────────
#
# 黑名单里每一类对应哪种真实写得出来的痕迹、以及为什么名单里没有光杆的「模板」，
# 见 `scripts/audit-rules.sh`。
hits="$(scan_re "$AUDIT_TRACE_RE" | grep -vE "$AUDIT_TRACE_ALLOW_RE" || true)"
if [ -n "$hits" ]; then
    report_and_die "template-trace" "$hits" \
        "随交付发布的文件里出现了只有模板维护者才能解析的记号。
   改法**不是**删掉它，是把它替换成它指向的事实本身——让没有设计稿的人读起来更顺，
   而不是读到一句缺了主语的话。"
fi
ok "template-trace（豁免：RFC 外部标准节号）"

# ── 5. 门禁的唯一入口 ───────────────────────────────────────────────────────
#
# 扫的是**文档里的第二入口**——那才是它实际的失败形态，理由见 `scripts/audit-rules.sh`。
hits="$(printf '%s\0%s\0' "$GEN_DEST/README.md" "$GEN_DEST/Makefile" \
    | xargs -0 grep -nHE -- "$AUDIT_ENTRY_RE" 2>/dev/null || true)"
if [ -n "$hits" ]; then
    report_and_die "single-gate-entry" "$hits" \
        "README 或 Makefile 里出现了 \`make check\` 之外的第二个门禁入口。
   一个需要人记得的门禁不是门禁：把它挂进 fmt / lint / test 三条之一里去。"
fi
grep -q 'make check' "$GEN_DEST/README.md" \
    || die "single-gate-entry：README.md 里一次都没提 \`make check\`——门禁的唯一定义必须写出来。"
ok "single-gate-entry"

gate_end
