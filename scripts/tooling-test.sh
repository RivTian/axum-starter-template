#!/usr/bin/env bash
# 门禁自己的单元测试：把每一条判据喂上合成输入，证明它**能红**。
#
#   scripts/tooling-test.sh [--quiet]
#
# ## 为什么门禁需要被测
#
# 其余每个入口都在回答"这棵树对不对"。它们全绿的时候，还剩一个问题没人回答：**判据本身
# 对不对**。一条永远匹配不上的正则、一份路径写错了的豁免清单、一个把第三方包名也换掉的
# 替换——这些东西的表现全都是"门禁是绿的"，和真的没问题长得一模一样。
#
# 所以这里的每一条都是**负向**的：构造一个本该被逮住的输入，断言它确实被逮住了；再构造
# 一个不该被逮住的，断言它确实放过了。两边都要，只验一边的检查还是可能是个常量。
#
# 覆盖的缺陷来源：B-21（未交付清单写死）、B-22（占位符按变量名枚举）、B-23（全量扫描不判
# 二进制）、B-24（历史标记正则全面禁用）、A-31（锁文件反向替换用前缀通配）、C-144（撞名
# 前缀）。
#
# ## 它不覆盖什么
#
# 判据与**主流程的接线**不在这里验：这条正则确实能逮住 X，不等于 `gen.sh` 真的拿它扫了
# 生成树。那件事由 `gen.sh` 自己每次跑真实的树来回答——两处合起来才完整，而把接线也搬进
# 合成输入意味着重写一遍主流程，那就成了测试测试自己。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=scripts/audit-rules.sh
. "$(dirname "${BASH_SOURCE[0]}")/audit-rules.sh"
# 只拿它的两个纯函数。它自己的 `BASH_SOURCE` 闸会挡住主流程——被 source 时不去生成树，
# 也不碰模板的 Cargo.lock。
# shellcheck source=scripts/normalize-lock.sh
. "$(dirname "${BASH_SOURCE[0]}")/normalize-lock.sh"

HERE="$(dirname "${BASH_SOURCE[0]}")"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "不认识的参数：$1" ;;
    esac
done

gate_covers "placeholder-generic"   "占位符扫描认的是形态不是变量名清单（B-22）"
gate_covers "placeholder-exempt"    "豁免按完整路径全等，不按文件名"
gate_covers "history-shape"         "历史标记正则是限定形态，不是 m<数字> 全面禁用（B-24）"
gate_covers "trace-allows-rfc"      "外部标准的节号不被当成设计稿痕迹"
gate_covers "undelivered-computed"  "未交付清单是算出来的，且基名撞车要滤掉（B-21）"
gate_covers "binary-skipped"        "全量扫描前先判文本/二进制（B-23）"
gate_covers "lock-third-party-kept" "锁文件反向替换不碰第三方同前缀包名（A-31）"
gate_covers "acceptance-extract"    "验收 ID 的抽取：散文里的 #[test] 不算，带参数的 #[tokio::test(…)] 要算（D54）"
gate_covers "prefix-collision"      "撞名前缀被 pre.rhai 当场拒绝（C-144）"
gate_begin  "tooling-test"

gate_workspace_init
gate_lock

W="$GATE_ROOT/tooling-test"
safe_rm_rf "$W"
mkdir -p -- "$W"

# ── 断言 ────────────────────────────────────────────────────────────────────
#
# 失败信息里一律带上**用的是什么输入**。一句"正则不匹配"要让人自己去猜是哪一行，
# 而猜错的成本是回来再跑一遍。

assert_eq() {  # $1 = 场景，$2 = 期望，$3 = 实际
    if [ "$2" != "$3" ]; then
        die "$1
   期望：$(printf '%s' "$2" | tr '\n' '|')
   实际：$(printf '%s' "$3" | tr '\n' '|')"
    fi
}

assert_match() {  # $1 = 场景，$2 = 正则，$3 = 文本
    printf '%s\n' "$3" | grep -qE -- "$2" \
        || die "$1：这一行**应当**被逮住，但正则放过了它。
   文本：$3"
}

assert_no_match() {  # $1 = 场景，$2 = 正则，$3 = 文本
    if printf '%s\n' "$3" | grep -qE -- "$2"; then
        die "$1：这一行**不该**被逮住，但正则匹配了它。
   文本：$3
   一条会误报的检查很快会被加上一长串豁免，再然后就没人看它了。"
    fi
}

assert_contains() {  # $1 = 场景，$2 = 定值子串，$3 = 文本
    printf '%s\n' "$3" | grep -qF -- "$2" || die "$1：输出里找不到 \"$2\"。"
}

assert_absent() {  # $1 = 场景，$2 = 定值子串，$3 = 文本
    if printf '%s\n' "$3" | grep -qF -- "$2"; then
        die "$1：输出里**不该**出现 \"$2\"，但它在。"
    fi
}

# ── placeholder-generic（B-22）──────────────────────────────────────────────
#
# 关键一条是那个**从来没人声明过**的变量名。B-22 的原形态是一张四个名字的清单，加第五个
# 变量时没人会记得回来补——而这里的正则对"第五个变量"和对已知的那四个反应完全一样。
assert_match    "placeholder-generic" "$AUDIT_PLACEHOLDER_RE" 'name = "{{ crate_prefix }}-core"'
assert_match    "placeholder-generic" "$AUDIT_PLACEHOLDER_RE" 'let x = {{ a_variable_nobody_declared }};'
assert_match    "placeholder-generic" "$AUDIT_PLACEHOLDER_RE" '{{crate_prefix}}'
# Rust 的格式串转义确实会被一起逮住——这不是缺陷，是那份豁免清单存在的理由。
assert_match    "placeholder-generic" "$AUDIT_PLACEHOLDER_RE" 'println!("{{}}");'
assert_no_match "placeholder-generic" "$AUDIT_PLACEHOLDER_RE" 'let s = format!("{x}");'
assert_no_match "placeholder-generic" "$AUDIT_PLACEHOLDER_RE" '一行普通的中文注释'
ok "placeholder-generic：未声明过的变量名照样被逮住；单层花括号放过"

# ── placeholder-exempt ──────────────────────────────────────────────────────
#
# 豁免是**完整路径**全等，不是文件名。换成按文件名匹配的话，任何一个新写的 `cli.rs` 都会
# 白白继承 `app/src/cli.rs` 的豁免，而继承来的豁免没有理由——那正是豁免清单要避免的事。
audit_placeholder_exempt "app/src/cli.rs"       || die "placeholder-exempt：清单里明写的路径反而没被豁免"
audit_placeholder_exempt "core/src/config/expand.rs" || die "placeholder-exempt：清单里明写的路径反而没被豁免"
if audit_placeholder_exempt "worker/src/cli.rs"; then
    die "placeholder-exempt：一个**不在**清单里的 cli.rs 被豁免了。
   说明匹配退化成了按文件名——新文件会白白继承别处的豁免，而那条豁免的理由跟它无关。"
fi
if audit_placeholder_exempt "src/cli.rs"; then die "placeholder-exempt：同上，路径前缀也被放过了"; fi
ok "placeholder-exempt：按完整路径全等，同名的别处文件不继承豁免"

# ── history-shape（B-24）────────────────────────────────────────────────────
#
# B-24 的原形态是 `(?<![a-z0-9])m\d+(?![a-z0-9])` 且忽略大小写，于是一个叫 `m1` 的局部变量
# 就能把门禁打红。这里收窄了两处，逐条钉住：大小写敏感、词边界是 `[^A-Za-z0-9_]`。
while IFS='|' read -r verdict text; do
    [ -n "$verdict" ] || continue
    case "$verdict" in
        catch) assert_match    "history-shape" "$AUDIT_HISTORY_RE" "$text" ;;
        pass)  assert_no_match "history-shape" "$AUDIT_HISTORY_RE" "$text" ;;
    esac
done <<'CASES'
catch|M3 的收尾在这里
catch|见 Phase 2 的产出
catch|GATE 1 之前不许自批
catch|闸门通过之后
catch|这是里程碑遗留下来的
catch|第三阶段的结论
catch|第一阶段先把分层定下来
catch|Apple M1 芯片上实测
pass|let m1 = compute();
pass|参数 m1 与 m2 互不影响
pass|mem3 是个环形缓冲区
pass|M3x 不是编号，是型号
pass|ARM64 与 M 系列芯片
pass|第三方依赖一律带理由
pass|一行没有任何编号的中文
CASES
ok "history-shape：大写+数字+词边界才算；小写 m1、mem3、M3x、第三方一律放过"

# 上面 `第三阶段` / `第一阶段` 那两条同时钉着一件跟中文无关的事：**正则里不许用多字节
# 字符类**。门禁跑在 `LC_ALL=C` 下（`scripts/lib.sh` 顶部写了为什么），C 把 `[…]` 当字节
# 集合，于是 `第[一二三四五六七八九]阶段` 要求中间恰好一个字节——三字节的中文数字永远进不
# 去。那条分支就是这么死过一次的，而死掉的分支和通过的检查在输出上完全一样。写成分支之后
# 比对的是定长字节串，换谁的语言环境都一样。

# 上面那条 `Apple M1` 是**已知且接受**的假阳性，写成 `catch` 是在钉住当前行为，不是在
# 表扬它。当前交付面里一个实例都没有；真撞上那天的表现是一条指名道姓的红，修法与占位符
# 那条一样：加一条带理由的豁免。在没有实例之前不预先铺那套机制。

# ── trace-allows-rfc ────────────────────────────────────────────────────────
#
# 设计稿痕迹那条黑名单会匹配任何 `§<数字>`，而 `RFC 9110 §15.5.9` 恰恰是这条纪律要保护的
# 反面——它是使用者查得到的外部标准。豁免走"从命中行里滤掉"，所以这里两步都要验。
assert_match "trace-allows-rfc" "$AUDIT_TRACE_RE" '错误信封见 §5.5'
assert_match "trace-allows-rfc" "$AUDIT_TRACE_RE" '语义见 RFC 9110 §15.5.9'
kept="$(printf '%s\n%s\n' '错误信封见 §5.5' '语义见 RFC 9110 §15.5.9' \
    | grep -E -- "$AUDIT_TRACE_RE" | grep -vE -- "$AUDIT_TRACE_ALLOW_RE" || true)"
assert_eq "trace-allows-rfc：豁免之后只该剩设计稿那一行" '错误信封见 §5.5' "$kept"
ok "trace-allows-rfc：RFC 节号被滤掉，设计稿节号留下"

# ── undelivered-computed（B-21）─────────────────────────────────────────────
#
# 三份合成清单。要证两件事：
#   ① 清单是**算**出来的——`docs/architecture.md` 和 `scripts/gen.sh` 谁都没在任何地方
#      声明过，它们仅仅因为"模板有、生成树没有"就进了名单；
#   ② 基名撞车要滤掉，而判据是"基名会不会撞车"**不是**"路径里有没有斜杠"。后者会把
#      `Makefile.project` 一起漏掉，而那正是真扑空的那一类。
cat > "$W/tpl.txt" <<'EOF'
.gitattributes
Makefile
Makefile.project
docs/architecture.md
scripts/gen.sh
src/main.rs
EOF
cat > "$W/gen.txt" <<'EOF'
Makefile
src/main.rs
EOF
cat > "$W/names.txt" <<'EOF'
Makefile
main.rs
EOF
sort -o "$W/tpl.txt" "$W/tpl.txt"
sort -o "$W/gen.txt" "$W/gen.txt"

raw="$(audit_undelivered_paths "$W/tpl.txt" "$W/gen.txt" "$W/names.txt" | sort)"
assert_eq "undelivered-computed：集合差" \
    "$(printf '%s\n' '.gitattributes' 'Makefile.project' 'docs/architecture.md' 'scripts/gen.sh' | sort)" \
    "$raw"

# 再走一遍豁免：`.gitattributes` 是 git 的公共约定，提它不会让人扑空。
final=""
while IFS= read -r p; do
    [ -n "$p" ] || continue
    audit_undelivered_exempt "$p" && continue
    final="$final$p"$'\n'
done <<EOF
$raw
EOF
assert_eq "undelivered-computed：豁免之后" \
    "$(printf '%s\n' 'Makefile.project' 'docs/architecture.md' 'scripts/gen.sh')" \
    "$(printf '%s' "$final" | sed '/^$/d')"
ok "undelivered-computed：没人声明过的路径照样进名单；Makefile 撞基名被滤，Makefile.project 保留"

# ── binary-skipped（B-23）───────────────────────────────────────────────────
#
# 二进制文件里**含有**一条会被历史标记正则逮住的字节串。判文本这一步要是没有，这条串会
# 让门禁报出一条谁也看不懂的红，而且换一份数据就没了——不可复现的假红比漏报更难处理。
mkdir -p -- "$W/tree"
perl -e 'print "M3 milestone\0trailing bytes\n"' > "$W/tree/asset.bin"
printf '%s\n' '这是一份普通的文本文件' > "$W/tree/note.md"

if is_text_file "$W/tree/asset.bin"; then
    die "binary-skipped：带 NUL 的文件被判成了文本。
   后果：全量扫描会去读它，而它里面那条 \"M3 milestone\" 会让历史标记检查报一条假红。"
fi
is_text_file "$W/tree/note.md" || die "binary-skipped：一份纯中文文本被判成了二进制。
   判据是「前 8 KiB 有没有 NUL」，UTF-8 里没有 NUL——这说明判别实现读错了字节。"

listed="$(list_text_files "$W/tree" | tr '\0' '\n' | sed "s|^$W/tree/||" | sort)"
assert_eq "binary-skipped：清单只含文本文件" "note.md" "$listed"
ok "binary-skipped：带 NUL 的资产不进扫描清单，纯中文文本进"

# ── lock-third-party-kept（A-31）────────────────────────────────────────────
#
# A-31 的形态：`sed 's/<前缀>-/{{crate_prefix}}-/g'`。前缀取 `tokio` 时第三方的 `tokio-util`
# 会被改写成 `{{crate_prefix}}-util`，锁文件当场坏掉，而坏法是静默的——仍然是合法 TOML，
# 仍然能提交，直到某个用户跑 `--locked` 才炸。
#
# 这份合成锁把最坏的情况摆齐了：前缀 `tokio` 本身是图里的一个包，`tokio-util` 与成员
# `tokio-core` 只差一个词，而且成员块与第三方块互相引用。
cat > "$W/lock.toml" <<'EOF'
[[package]]
name = "tokio"
version = "1.53.1"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "tokio-util"
version = "0.7.16"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "tokio",
]

[[package]]
name = "tokio-core"
version = "0.1.0"
dependencies = [
 "tokio",
 "tokio-util",
]

[[package]]
name = "tokio-app"
version = "0.1.0"
dependencies = [
 "tokio-core",
 "tokio-util",
]
EOF

locals="$(lock_local_packages "$W/lock.toml")"
assert_eq "lock-third-party-kept：本地包判据是结构的（没有 source 行）" \
    "$(printf '%s\n' 'tokio-app' 'tokio-core')" "$locals"

rewritten="$(lock_denormalize "$W/lock.toml" "tokio")"
# 成员被换掉了。
assert_contains "lock-third-party-kept" 'name = "{{crate_prefix}}-core"' "$rewritten"
assert_contains "lock-third-party-kept" 'name = "{{crate_prefix}}-app"'  "$rewritten"
assert_contains "lock-third-party-kept" '"{{crate_prefix}}-core",'       "$rewritten"
# 第三方一个字节都没动。下面这条 `{{crate_prefix}}-util` 就是 A-31 的签名：它一旦出现，
# 说明替换又退化成了前缀通配。
assert_absent   "lock-third-party-kept" '{{crate_prefix}}-util'          "$rewritten"
assert_contains "lock-third-party-kept" 'name = "tokio-util"'            "$rewritten"
assert_contains "lock-third-party-kept" 'name = "tokio"'                 "$rewritten"
assert_absent   "lock-third-party-kept" '"tokio-core"'                   "$rewritten"
assert_absent   "lock-third-party-kept" '"tokio-app"'                    "$rewritten"
# 裸引用 `"tokio",` 必须还在：它是第三方包在 dependencies 列表里的写法。
assert_contains "lock-third-party-kept" ' "tokio",'                      "$rewritten"
ok "lock-third-party-kept：tokio-util / tokio 未被触碰，只有两个成员被换成占位符"

# ── acceptance-extract（D54 / D55）──────────────────────────────────────────
#
# `acceptance-ids` 那条门禁的全部力量来自一件事：两侧集合必须**各自抽得准**。抽多了，
# 文档就得补一个不存在的 ID 才能变绿——门禁开始逼人写假条目；抽少了，新用例永远不需要
# 被声称——门禁对它一无所知，还是绿的。两个方向都是静默的，所以只能在这里用合成输入验。
#
# 两个方向在这棵树上都真的发生过，数字记在这里：真实用例 **264**，而
# `grep -c '#\[test\]'` 数出 192、加上 `#[tokio::test]` 两个字面量数出 255、放宽成前缀
# 数出 267（264 + 3 处散文）。多数的那三处是散文（下面第一个函数名就是那个形态）；少数
# 的那 12 条是 `#[tokio::test(start_paused = true)]` 之类**带参数**的属性——第一版判据
# 要求属性恰好是那两个字面量，于是它们整批消失，而两侧仍然相等、门禁仍然全绿。
accept_dir="$W/accept"
mkdir -p -- "$accept_dir"

cat > "$accept_dir/a.rs" <<'RS'
//! 合成输入，不参与编译。

/// 散文里提到属性的写法，例如：
/// #[test]
/// fn prose_mentioned_name() {}
/// 上面三行是说明，不是用例。
#[test]
fn plain_form() {}

/// 真实形态，也是最危险的一种：文档注释的**最后一行**提到属性，下一行就是一个真的函数。
/// 树上那两处（`worker/tests/ticker.rs` 的 republish_interval、`api/tests/contract.rs`
/// 的 send）逐字就长这样，只认 `#[test]` 函数体和 `#[cfg(test)]` 模块。
fn doc_commented_helper() {}

#[tokio::test]
async fn tokio_form() {}

#[tokio::test(start_paused = true)]
async fn paused_param_form() {}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_thread_param_form() {}

#[test] // 尾随注释：rustfmt 保留它，所以这个形态在绿树里可达
fn trailing_comment_form() {}

#[test]
#[should_panic]
#[ignore]
fn interleaved_attrs_form() {}

#[test]

fn blank_line_between() {}

#[test]
let _x = 1;
fn code_between() {}

// #[test]
fn commented_out_attr() {}

#[cfg(test)]
fn not_a_test() {}

#[test_case(1, 2)]
fn not_the_test_attr() {}
RS

got="$(audit_test_names "$accept_dir/a.rs" | cut -f1)"
assert_eq "acceptance-extract：抽到的名字集合" \
    "$(printf '%s\n' plain_form tokio_form paused_param_form multi_thread_param_form \
        trailing_comment_form interleaved_attrs_form)" \
    "$got"
# 逐条点名，是为了让失败信息直接说出**哪一个形态**退化了；上面的全等断言只会说"不一样"。
assert_contains "acceptance-extract：带参数的 tokio 属性（漏掉的那 12 条）" 'paused_param_form'       "$got"
assert_contains "acceptance-extract：带参数且参数里有逗号"                   'multi_thread_param_form' "$got"
assert_absent   "acceptance-extract：散文里的 #[test]（多数那一侧的来源）"   'prose_mentioned_name'    "$got"
assert_absent   "acceptance-extract：#[test_case] 不是测试属性，只是前缀像"  'not_the_test_attr'       "$got"
assert_absent "acceptance-extract：文档注释提到 #[test]，下一行是真函数"   'doc_commented_helper' "$got"
assert_absent "acceptance-extract：属性与 fn 之间夹空行"                   'blank_line_between'   "$got"
assert_absent "acceptance-extract：属性与 fn 之间夹代码"                   'code_between'         "$got"
assert_absent "acceptance-extract：被注释掉的属性"                         'commented_out_attr'   "$got"
assert_absent "acceptance-extract：cfg(test) 不是用例属性"                 'not_a_test'           "$got"

# 输出的第二列是路径。`acceptance-ids` 报"源码有、文档无"时要靠它指人去看，指错文件比
# 不指更费时间——所以跨文件复位要验：上一个文件停在 armed 状态，不能算到下一个文件头上。
printf '%s\n' '#[test]' > "$accept_dir/b.rs"
printf '%s\n' 'fn orphan_first_line() {}' > "$accept_dir/c.rs"
cross="$(audit_test_names "$accept_dir/b.rs" "$accept_dir/c.rs" | cut -f1)"
assert_absent "acceptance-extract：跨文件复位" 'orphan_first_line' "$cross"
assert_contains "acceptance-extract：第二列是出处路径" \
    "$accept_dir/a.rs" "$(audit_test_names "$accept_dir/a.rs")"

# 判据对属性里写了什么一概不管之后，只剩一种它**认不了**的形态：参数被折成多行。认不了
# 必须是红的。静静跳过就退回到那 12 条凭空消失的老毛病上——而那个方向不会有人发现。
printf '%s\n' '#[tokio::test(' '    flavor = "multi_thread",' '    worker_threads = 2' ')]' \
    'async fn wrapped_attr_form() {}' > "$accept_dir/d.rs"
if audit_test_names "$accept_dir/d.rs" >"$W/accept-d.out" 2>"$W/accept-d.err"; then
    die "acceptance-extract：属性跨行时抽取器认不了它，必须非零退出；实际静静地过了"
fi
assert_contains "acceptance-extract：跨行属性的诊断说出文件与行号" \
    "d.rs 第 1 行" "$(cat "$W/accept-d.err")"
assert_eq "acceptance-extract：跨行属性也不会被半抽出来" "" "$(cat "$W/accept-d.out")"

# 文档侧：条目是行首的破折号加反引号包住的名字，散文里提到用例名不是条目。少了这条区分，
# 验收文档就不能用散文解释任何一条用例——而一份只有清单没有解释的文档，没人读得懂它在验
# 什么。
assert_match    "acceptance-extract" "$AUDIT_ACCEPTANCE_ID_RE" '- `a_real_entry` —— 说明'
assert_no_match "acceptance-extract" "$AUDIT_ACCEPTANCE_ID_RE" '这一条由 `inline_mention` 证明。'
assert_no_match "acceptance-extract" "$AUDIT_ACCEPTANCE_ID_RE" '  - `indented_entry`'
assert_no_match "acceptance-extract" "$AUDIT_ACCEPTANCE_ID_RE" '- 一条没有反引号的普通条目'
ok "acceptance-extract：六种可达形态抽全（含带参数的 tokio 属性），散文/空行/夹代码/注释掉的属性不算，跨行属性当场红"

# ── prefix-collision（C-144）────────────────────────────────────────────────
#
# 前面那些验的都是脚本里的判据。这一条验的是**钩子**：撞名前缀必须在生成开始之前就被
# 拒绝，而不是等到用户第一次跑 `--locked` 才炸在一条讲锁文件的报错上。
#
# `structure.sh::name-collision` 已经在比对"钩子拒绝的清单"与"锁文件反推出来的集合"——
# 那验的是清单**对不对**。这里验的是清单**生不生效**：两份一模一样的名字，一个真的走一趟
# cargo-generate。少了这一条，钩子里的 `reject` 被人注释掉时没有任何东西会红。
collide_dir="$W/collide"
mkdir -p -- "$collide_dir"
set +e
( cd "$collide_dir" && cargo generate --path "$TEMPLATE_ROOT" --name axum --force ) \
    > "$W/collide.log" 2>&1
collide_status=$?
set -e

if [ "$collide_status" -eq 0 ]; then
    die "prefix-collision：前缀 \`axum\` **竟然**生成成功了。
   hooks/pre.rhai 的 collides_with_dependency 没有生效。
   后果：发出去的 Cargo.lock 用裸名字写依赖引用，而两个包同名时 cargo 改用
   \`name version\` 的消歧形式——生成结果的第一条命令就会失败，报错还只讲锁文件。"
fi
grep -q 'collides with a package already in the' "$W/collide.log" || {
    sed 's/^/   /' "$W/collide.log"
    die "prefix-collision：生成确实失败了，但失败的不是那条 reject（见上）。
   一个碰巧失败的生成不能当成钩子生效的证据。"
}
# cargo-generate 0.24.0 **先建目标目录再跑前置钩子**，所以被拒之后地上会留下一个空目录。
# 该断言的不是"目录不存在"——那不成立，而是"没有任何文件落盘"：用户手上不能剩半棵树。
left="$(find "$collide_dir" -type f | wc -l | tr -d ' ')"
[ "$left" = 0 ] || {
    find "$collide_dir" -type f | sed 's/^/   /'
    die "prefix-collision：被拒绝了，却已经写下了 $left 个文件（见上）。
   前置钩子必须在**任何文件落盘之前**拒绝，否则用户手上会剩半棵树，
   而半棵树比没有更糟：它看起来像是能接着用的。"
}
# 那个空目录之所以无害，全靠拒绝信息里那句"先删掉它再重试"。少了这句，用户下一次运行
# 撞上的是 cargo-generate 的 `Target directory already exists`——一条完全不提撞名的报错。
grep -q 'remove it before retrying' "$W/collide.log" \
    || die "prefix-collision：拒绝信息里没有「先删掉那个空目录」的提示。
   cargo-generate 已经把目录建出来了，用户重试时会撞上一条与撞名无关的报错。"
ok "prefix-collision：前缀 axum 被前置钩子拒绝，零文件落盘，且提示了如何重试"

safe_rm_rf "$W"
gate_end
