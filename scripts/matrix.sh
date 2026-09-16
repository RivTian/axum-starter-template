#!/usr/bin/env bash
# 名字矩阵：四组边界名字各走一遍完整的「生成 + 审计 + 结构」。
#
#   scripts/matrix.sh [--quiet]
#
# ## 为什么需要它
#
# 其余每一条门禁都只在**一个**名字上跑（默认那个）。于是任何"只在某类名字下才错"的缺陷
# 都是全绿的。已经踩到过的两个形状：
#
#   - 不带连字符的前缀让 `{{crate_prefix}}` 和它的派生形态（大写、连字符换下划线）渲染
#     结果完全相同。写错变体在门禁里**不可分辨**，而用户选一个带连字符的名字就炸。
#   - 一个撞了依赖图里已有包名的前缀，会让随模板发布的 `Cargo.lock` 整个失配。它在
#     默认名字下永远看不见，而用户敲的第一条命令就红。
#
# 所以这四组不是"多跑几遍图个心安"，是四类各自能单独失败的边界。
#
# ## 四组是怎么选出来的
#
# | 组 | 名字 | 它让什么第一次可分辨 |
# | --- | --- | --- |
# | 带连字符 | `orders-gateway` | 派生占位符（`upcase` + `replace`）与恒等形态分开；环境变量前缀第一次不等于项目名 |
# | 极长 | 50 字符 | 所有靠空格补齐的列；嵌进包名之后的路径长度 |
# | 极短 | `x` | 单字符前缀；任何"名字至少两个字符"的隐含假设 |
# | 与 crates.io 撞名 | `tokio` | 本地包名遮蔽一个图里已有的 crate——但六个成员名都不在锁里，所以它**应当通过** |
#
# 最后一组的重点在"应当通过"。真正会失配的那五个名字（`axum` / `futures` / `sqlx` /
# `sqlx-macros` / `tracing`）由 `hooks/pre.rhai` 在生成之前就拒掉，那条拒绝的负向用例在
# `scripts/tooling-test.sh` 里。这里要证的是反面：**拒绝面没有放大**——一个只是碰巧和
# 知名 crate 同名、但并不造成撞名的前缀，必须照常能用。
#
# ## 它比 `structure` 多验什么
#
# 每组跑完 `gen` + `structure` 之后，额外验两条**只有换名字才可能红**的事：
#
#   1. 环境变量前缀：渲染面（Liquid 的 `upcase | replace`）与运行期（Rust 的
#      `env_prefix`，按 ASCII 字母数字大写、其余换 `_`）算出来的必须是同一个串。
#      两边的实现完全独立，而它们在默认名字下恰好都对——所以只有带连字符那一组能分辨。
#   2. 列对齐：没有任何一行把项目名后面接一段用于补齐的空格。补齐的列会随名字一起歪，
#      而极长那一组会歪到读不了。
#
# 其余结构性事实一律交给 `structure`，这里不重复。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HERE="$(dirname "${BASH_SOURCE[0]}")"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "不认识的参数：$1" ;;
    esac
done

# 极长的那一组**算出来再断言长度**，不写死一个数出来的串。写死的代价是有人顺手改了它
# 之后长度不再是 50，而"这一组还算不算极长"没有任何东西会说话。
LONG_NAME="a-very-long-project-name-that-keeps-on-going-there"
[ "${#LONG_NAME}" -eq 50 ] || die "极长组的名字应当是 50 字符，实际 ${#LONG_NAME}：$LONG_NAME
   （它要压住 README 的列对齐与包名长度这两件事，长度本身就是判据的一部分）"

# 名字与它代表的那一类边界。顺序即执行顺序。
NAMES="orders-gateway $LONG_NAME x tokio"

gate_covers "hyphen"        "带连字符的前缀：派生占位符与恒等形态可分辨（orders-gateway）"
gate_covers "long"          "极长前缀：50 字符，压住列对齐与路径长度"
gate_covers "short"         "极短前缀：单字符"
gate_covers "crates-io"     "与知名 crate 同名但不撞锁的前缀必须照常能用（tokio）"
gate_covers "env-prefix"    "渲染面与运行期算出来的环境变量前缀相同（每组一次）"
gate_covers "no-padding"    "没有任何一行在项目名后面补空格对齐（每组一次）"
gate_begin  "matrix"

gate_workspace_init
# 整个矩阵只锁一次。四组里的 `gen` / `structure` 是本进程的子进程，它们看到
# `GATE_LOCK_HELD` 就不再自己去抢——见 `lib.sh::gate_lock` 的可重入说明。
gate_lock

# 逐组的检查。`$1` = 项目名。
check_one() {
    local name="$1"
    local dest="$GATE_ROOT/gen/$name"
    local prefix expected_var got_vars padded

    bash "$HERE/gen.sh"       --name "$name" --quiet
    bash "$HERE/structure.sh" --name "$name" --quiet

    # 前缀从生成树里读，不从 `$name` 推：`pre.rhai` 会做规范化，推一遍就是把那段逻辑
    # 抄进门禁——然后两份会不一致。与 `structure.sh` 同一条理由。
    prefix="$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\(.*\)-core"[[:space:]]*$/\1/p' \
        "$dest/core/Cargo.toml" | head -1)"
    [ -n "$prefix" ] || die "matrix：从 $dest/core/Cargo.toml 里读不出包名前缀"

    # ── env-prefix ───────────────────────────────────────────────────────────
    #
    # 三份实现描述同一个转换，门禁在这里当裁判：
    #
    #   渲染面  `README.project.md` 的 `{{crate_prefix | upcase | replace: "-", "_" }}`
    #   运行期  `app/src/config/bootstrap.rs::env_prefix`（ASCII 字母数字大写，其余换 `_`）
    #   这里    `tr 'a-z-' 'A-Z_'`
    #
    # 三者在 `pre.rhai` 允许的字符集 `[a-z0-9-]` 上完全一致——这不是巧合，是那条字符集
    # 限制让它们一致的。运行期那份还多覆盖了字符集之外的输入（`a.b c` → `A_B_C`），它有
    # 自己的单元测试；这里只管可能真的被生成出来的那些名字。
    expected_var="$(printf '%s' "$prefix" | tr 'a-z-' 'A-Z_')_CONFIG"

    # 判据是**集合相等**而不是"含有"：含有放得过"README 里同时留着上一版的变量名"这种
    # 情况——新的那个在，旧的那个也在，而读的人会试旧的那个。
    got_vars="$(grep -oh '[A-Z0-9_]*_CONFIG' "$dest/README.md" | sort -u | tr '\n' ' ')"
    got_vars="${got_vars% }"
    [ "$got_vars" = "$expected_var" ] || die "env-prefix（$name）：README 里的配置变量名不对。
   期望（唯一一个）：$expected_var
   实际：           ${got_vars:-（一个都没有）}
   渲染面用的是 Liquid 的 \`upcase | replace\`，运行期用的是 Rust 的 \`env_prefix\`。
   两边在默认名字下恰好相同，只有带连字符的名字能把它们分开——也就是说这条红意味着
   有一侧从来就是错的，只是到今天才看得见。"

    # ── no-padding ───────────────────────────────────────────────────────────
    #
    # 拦的是"名字后面跟着一段用于对齐的空格"。补齐的列宽是按某一个具体名字调出来的，
    # 换个长度就歪；极长那一组会歪到一行读不完。
    #
    # 判据写成"项目名（连同紧跟的非空白）之后出现两个以上空格，且后面还有内容"——单个
    # 空格是正常分词，行尾空格由 fmt 管，都不在这条里。
    #
    # 两处收窄，各自拦一种假红：
    #
    #   1. **只扫渲染面**。逐字节复制过来的文件里不含项目名，里面出现的同名子串是巧合，
    #      而且它们的内容根本不随名字变化——不可能歪。不收窄的话 `clippy.toml` 里的
    #      `axum::extract` 会在极短名那一组里被当成"项目名后面补了空格"。
    #   2. **要求词边界**。极短组的前缀是单字符，裸子串匹配会命中 `expect_used`、
    #      `unused_extern_crates`、`0002_xxx.sql` 这一类——五条全是假的。
    #
    # 渲染面的定义与 `structure.sh::byte-identical` 同源：`Cargo.toml` / `Cargo.lock` /
    # `README.md` / `Makefile`。那里用它来决定"跳过哪些"，这里用它来决定"只看哪些"。
    padded="$(find "$dest" \( -name .git -o -name target \) -prune -o -type f \
            \( -name 'Cargo.toml' -o -name 'Cargo.lock' -o -name 'README.md' -o -name 'Makefile' \) -print0 \
        | xargs -0 grep -nHE -- "(^|[^A-Za-z0-9_-])$prefix[^ ]*  +[^ ]" 2>/dev/null || true)"
    if [ -n "$padded" ]; then
        die "no-padding（$name）：项目名后面有用于补齐的空格。
$(printf '%s\n' "$padded" | sed 's/^/   /')
   把变长的那一项挪到行尾，或者干脆不补齐。列宽是按一个具体名字调出来的，
   而项目名的长度是用户定的。"
    fi

    # 这一组过了就把它的树删掉：四棵树同时留着只是占地方，而失败那一棵会因为 `die`
    # 直接跳过这一行留在原地——需要现场的时候它就在。
    safe_rm_rf "$dest"
}

for n in $NAMES; do
    check_one "$n"
    ok "$n（${#n} 字符）：gen + structure + env-prefix + no-padding"
done

gate_end
