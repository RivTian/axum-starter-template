#!/usr/bin/env bash
# `docs/acceptance.md` 的 ID 集合与源码里的用例名集合必须**完全相等**（D54 / D55）。
#
#   scripts/acceptance-ids.sh [--quiet]
#
# ## 这条门禁挡的是什么
#
# X-01：上一版把 55 个验收 ID 写进文档，拿去 `.rs` 里搜零命中。那套编号和代码之间没有任何
# 机械联系，所以它可以在文档里活得好好的，而对应的用例早就改名或者压根没写过。文档因此
# 变成一份**声称**——读的人没法分辨哪几条是真的。
#
# 修法不是"再认真一点"，是让 ID 就是函数名：改名即改 ID，两侧当场不等。
#
# ## 为什么两个方向都要查
#
# 只查"文档里的在不在源码里"，挡得住假条目，挡不住**漏声称**：新加了一条用例却没归到任何
# 一项能力名下，文档依然全绿，而它描述的覆盖面已经不是真的了。反过来只查源码侧则挡不住
# 假条目。两个方向合起来才是"相等"。
#
# 多出来的用例算错误，这一条偶尔会让人意外。理由是：一条不属于任何已声称能力的用例，要么
# 说明能力漏写了，要么说明它在验一件没人想验的事——两种情况都需要人看一眼，而不是让门禁
# 替他决定哪种无所谓。
#
# ## 抽取按行锚定
#
# 判据在 `scripts/audit-rules.sh::audit_test_names`，与 `tooling-test` 共用同一份定义。
# 它要求测试属性**独占一行**，因为这棵树上真有三处 `#[test]` 写在文档注释的散文里
# （`worker/tests/ticker.rs` 与 `api/tests/contract.rs` 讲 `clippy.toml` 的结构性豁免）。
#
# 这棵树上几种数法的结果（真实用例 **264**）：
#
#   `grep -c '#\[test\]'`                     192   —— 漏掉全部 `#[tokio::test]`
#   `#[test]` / `#[tokio::test]` 两个字面量   255   —— 多数 3 处散文，漏 12 条带参数的
#   `#[test` / `#[tokio::test` 前缀           267   —— 264 条真用例 + 3 处散文
#
# 三个数都不等于 264，而且错在**两个相反的方向**上。多数的那头逼人往文档里补一个不存在
# 的 ID 才能变绿；少数的那头更坏——抽不到的用例永远不需要出现在文档里，门禁对它一无所知
# 还是绿的。判据因此按行锚定（挡住散文），并且对属性里写了什么一概不管（认下带参数的）。
#
# ## 为什么没有"重新生成 acceptance.md"的目标
#
# 因为那个目标会让这条门禁的值变成零：所有人都会先跑它再看红绿，于是两侧永远相等，而
# "永远相等"和"没有检查"是同一件事。清单只在建档时被机械生成过一次，之后手工维护。
# 这一点同样写在 `docs/acceptance.md` 的开头，改那份文件的人第一屏就会看到。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=scripts/audit-rules.sh
. "$(dirname "${BASH_SOURCE[0]}")/audit-rules.sh"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "acceptance-ids.sh: 不认识的参数：$1" ;;
    esac
done

gate_covers "doc-shape"      "文档侧条目形态合法，且没有一个用例被两节重复认领"
gate_covers "ids-equal"      "文档 ID 集合与源码用例名集合完全相等（两个方向都查）"
gate_covers "section-counts" "每节标题里写的数量与该节实际条目数一致"
gate_begin  "acceptance-ids"

gate_workspace_init

DOC="$TEMPLATE_ROOT/docs/acceptance.md"
[ -f "$DOC" ] || die "acceptance-ids：找不到 $DOC。
   这份清单是 D54 / D55 的落点，不是可选文档。"

W="$GATE_ROOT/acceptance"
safe_rm_rf "$W"
mkdir -p "$W"

# ── 文档侧 ──────────────────────────────────────────────────────────────────
grep -E "$AUDIT_ACCEPTANCE_ID_RE" "$DOC" \
    | sed -E 's/^- `([A-Za-z0-9_]+)`.*/\1/' > "$W/doc.raw"

doc_raw="$(wc -l < "$W/doc.raw" | tr -d ' ')"
[ "$doc_raw" -gt 0 ] || die "acceptance-ids：$DOC 里一个条目都没抽到。
   条目形态是行首的 - \`name\`（反引号包住函数名）。
   形态变了的话，判据在 scripts/audit-rules.sh::AUDIT_ACCEPTANCE_ID_RE。"

sort "$W/doc.raw" > "$W/doc.sorted"
sort -u "$W/doc.raw" > "$W/doc.ids"

if ! dup="$(uniq -d "$W/doc.sorted")" || [ -n "$dup" ]; then
    die "acceptance-ids：$DOC 里有重复条目：
$(printf '%s\n' "$dup" | sed 's/^/     /')
   一个用例只能被一项能力认领。重复会让「两侧相等」这个结论在去重之后依然成立，
   于是「这条用例到底证明哪项能力」就没有答案了。"
fi
ok "doc-shape：$doc_raw 条条目，无重复"

# ── 源码侧 ──────────────────────────────────────────────────────────────────
find "$TEMPLATE_ROOT" -name '*.rs' -not -path '*/target/*' | sort > "$W/files"
[ -s "$W/files" ] || die "acceptance-ids：模板树里一个 .rs 都没找到，抽取无从谈起。"

RS_FILES=()
while IFS= read -r f; do RS_FILES+=("$f"); done < "$W/files"

audit_test_names "${RS_FILES[@]}" > "$W/src.tsv"
cut -f1 "$W/src.tsv" | sort > "$W/src.sorted"
sort -u "$W/src.sorted" > "$W/src.ids"

src_raw="$(wc -l < "$W/src.sorted" | tr -d ' ')"
src_uniq="$(wc -l < "$W/src.ids" | tr -d ' ')"
if [ "$src_raw" != "$src_uniq" ]; then
    die "acceptance-ids：源码里有同名用例（$src_raw 条里只有 $src_uniq 个不同的名字）：
$(uniq -d "$W/src.sorted" | sed 's/^/     /')
   名字当 ID 的前提是名字唯一。重名的两条用例没法在文档里分别认领。"
fi

# ── 两个方向的差 ────────────────────────────────────────────────────────────
comm -23 "$W/doc.ids" "$W/src.ids" > "$W/only-doc"
comm -13 "$W/doc.ids" "$W/src.ids" > "$W/only-src"

if [ -s "$W/only-doc" ] || [ -s "$W/only-src" ]; then
    msg="acceptance-ids：两侧 ID 集合不等。"
    if [ -s "$W/only-doc" ]; then
        msg="$msg
   ── 文档里有、源码里没有（$(wc -l < "$W/only-doc" | tr -d ' ') 条）──
$(sed 's/^/     /' "$W/only-doc")
   这些是**声称**：文档说有这条用例，源码里搜不到。要么用例改名了（把文档改成新名字），
   要么它从来没写过（把这一行删掉，或者去把用例补上）。X-01 记的就是这种。"
    fi
    if [ -s "$W/only-src" ]; then
        msg="$msg
   ── 源码里有、文档里没有（$(wc -l < "$W/only-src" | tr -d ' ') 条）──
$(sed 's/^/     /' "$W/only-src")
   这些用例不属于任何一项已声称的能力。要么 docs/acceptance.md 里缺了对应的能力段落，
   要么这条用例在验一件没被声称的事。两种都要人看一眼。"
    fi
    die "$msg"
fi
ok "ids-equal：两侧各 $src_uniq 个 ID，集合相等"

# ── 每节标题里的数量 ────────────────────────────────────────────────────────
#
# 标题写成 `## N. 标题（COUNT）`。这一条挡的是"加了条目但没改标题"——不致命，但那个数字
# 是读的人唯一能快速核对的东西，错了比没有更坏。
perl -ne '
    if (/^##\s+(\d+)\..*（(\d+)）/) {
        if (defined $sec) { print "$sec\t$want\t$have\n" }
        ($sec, $want, $have) = ($1, $2, 0);
    } elsif (/^##\s/) {
        if (defined $sec) { print "$sec\t$want\t$have\n" }
        undef $sec;
    } elsif (defined $sec && /^- `[A-Za-z0-9_]+`/) {
        $have++;
    }
    END { if (defined $sec) { print "$sec\t$want\t$have\n" } }
' "$DOC" > "$W/sections"

[ -s "$W/sections" ] || die "acceptance-ids：$DOC 里没抽到任何带数量的节标题。
   形态是 ## N. 标题（COUNT）——全角括号。"

bad="$(awk -F'\t' '$2 != $3 { printf "     第 %s 节：标题写 %s 条，实际 %s 条\n", $1, $2, $3 }' "$W/sections")"
[ -z "$bad" ] || die "acceptance-ids：节标题里的数量对不上。
$bad"

sec_total="$(awk -F'\t' '{ s += $3 } END { print s+0 }' "$W/sections")"
[ "$sec_total" = "$doc_raw" ] || die "acceptance-ids：各节条目数之和是 $sec_total，
   但全文抽到 $doc_raw 条——有条目落在任何一节之外（多半在 ## 标题上面）。"
ok "section-counts：$(wc -l < "$W/sections" | tr -d ' ') 节，合计 $sec_total 条，与标题一致"

gate_end
