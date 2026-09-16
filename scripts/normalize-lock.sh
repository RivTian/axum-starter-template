#!/usr/bin/env bash
# 重新生成模板自己的 `Cargo.lock`。
#
#   scripts/normalize-lock.sh [--quiet]
#
# **这是唯一一个往模板仓库里写文件的脚本。** 其余每一个都只读模板、只写 `$GATE_ROOT`。
#
# ## 为什么不能就地 `cargo generate-lockfile`
#
# 模板根的 `Cargo.toml` 里写着 `{{crate_prefix}}-core`，cargo 解析不了——它连清单都读不
# 完。所以流程必须绕一圈：
#
#   展开成一个具体名字的工作区 → 在那里让 cargo 解析 → 把成员名反向替换回占位符 → 落回来
#
# ## 反向替换为什么不能用 `sed 's/<前缀>-/{{crate_prefix}}-/g'`
#
# 因为那个 `-` 后面跟的是什么，`sed` 不知道。前缀取 `tokio` 时 `tokio-util`（第三方）会被
# 改成 `{{crate_prefix}}-util`，锁文件当场坏掉，而坏法是**静默**的：文件仍然是合法 TOML，
# 仍然能提交，直到某个用户跑 `--locked` 才炸。
#
# 这里的判据是**结构**的，不是名字的：`Cargo.lock` 里一个 `[[package]]` 块**没有
# `source =` 行**，就说明它是本地路径包。只替换这些块的名字，以及别处对这些名字的引用
# ——而且是整个带引号的字符串全等匹配，不是前缀匹配。于是"第三方包碰巧和某个成员同名"
# 这件事不可能发生在替换里：它会先在下面那条相等断言上红。
#
# ## 为什么不顺手把依赖升级了
#
# 这个脚本只做 `cargo generate-lockfile`：清单说什么，锁就跟到什么。**不跑
# `cargo update`**。升级依赖是一次有意的、要逐条看 diff 的动作；和"清单改了、锁要跟上"
# 共用一个命令的话，一次本该只有三行的改动会变成一整页，而那一页没有人真的读。
# 要升级就去改清单里的版本要求，锁自然跟上——改动因此有了理由，也留在了历史里。
#
# ## 为什么没有 `--check` 模式
#
# "锁文件是不是过期了"已经有人管：`gen` 之后的 `project-check` 在生成工程里跑
# `cargo test --workspace --locked`，清单和锁对不上时 `--locked` 会当场拒绝。再写一个
# 只在 CI 里跑一遍的等价检查，就是多一个会和它漂移的实现。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# 六个成员的层名，与 `structure.sh` 同一份。
LOCK_MEMBERS="core storage worker api app testkit"

# 解析用的一次性名字。它只在 `$GATE_ROOT` 里活几秒钟，不会出现在任何交付物里。
# 取一个不像 crate 的串：它一旦和依赖图里某个包撞名，下面那条相等断言就会红——
# 而那条红说的正是这件事。
#
# **改它要付一次代价，所以别顺手改。** cargo 按名字字典序写 `[[package]]` 块，也按名字
# 字典序写块内的 `dependencies` 列表。六个成员在锁文件里的位置因此完全由这个串决定——
# 换一个首字母不同的名字，六个块连同十几处引用会整体挪位，而**解析结果一个字节都没变**。
# 那种 diff 读不出任何信息，却会盖住真正的依赖变动。实测过一次：从 `demo-svc` 换到
# `tplxlock`，156 行纯位移。
LOCK_SCRATCH="tplxlock"

# ── 可被单元测试调用的两个纯函数 ────────────────────────────────────────────

# 列出锁文件里的**本地包**：`[[package]]` 块里没有 `source =` 行的那些。
#
# 块之间以空行分隔，这是 cargo 自己的写法。`END` 那一支是给"文件不以空行结尾"准备的。
lock_local_packages() {
    awk '
        /^\[\[package\]\]/ { inblk = 1; name = ""; src = 0; next }
        inblk && /^name = "/ { name = $0; sub(/^name = "/, "", name); sub(/"$/, "", name) }
        inblk && /^source = / { src = 1 }
        /^$/ { if (inblk && name != "" && src == 0) print name; inblk = 0; name = ""; src = 0 }
        END   { if (inblk && name != "" && src == 0) print name }
    ' "$1" | sort -u
}

# 把 `<前缀>-<成员>` 换回 `{{crate_prefix}}-<成员>`，读 stdin 或文件、写 stdout。
#
# 全等匹配整个带引号的字符串。`"tplxlock-core"` 会被换，`"tplxlock-coreutils"` 不会，
# `"tokio-util"` 在任何前缀下都不会。
lock_denormalize() {  # $1 = 锁文件路径，$2 = 展开时用的前缀
    local script="" m
    for m in $LOCK_MEMBERS; do
        script="$script;s/\"$2-$m\"/\"{{crate_prefix}}-$m\"/g"
    done
    sed "${script#;}" "$1"
}

# ── 主流程 ──────────────────────────────────────────────────────────────────

lock_main() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --quiet) GATE_QUIET=1; shift ;;
            -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
            *) die "不认识的参数：$1" ;;
        esac
    done

    gate_covers "resolve"    "在展开出来的工作区里让 cargo 解析出一份锁"
    gate_covers "local-set"  "锁里的本地包集合 == 六个成员，相等比较"
    gate_covers "denormalize" "成员名换回占位符；第三方名字一个都不许动"
    gate_covers "round-trip" "拿新锁重新生成一次，--locked 必须认"
    gate_begin  "lock"

    gate_workspace_init
    gate_lock

    local here dest before_count after_count
    here="$(dirname "${BASH_SOURCE[0]}")"
    dest="$GATE_ROOT/gen/$LOCK_SCRATCH"

    if [ -f "$TEMPLATE_ROOT/Cargo.lock" ]; then
        before_count="$(grep -c '^\[\[package\]\]' "$TEMPLATE_ROOT/Cargo.lock" || true)"
    else
        before_count=0
    fi

    # ── resolve ─────────────────────────────────────────────────────────────
    #
    # 走 `gen.sh` 而不是自己拷一遍模板树：渲染规则只该有一处实现。顺带地，这一趟也把
    # `gen` 的那几条审计跑了——一个连自己的审计都过不了的模板，不该先去更新锁文件。
    bash "$here/gen.sh" --name "$LOCK_SCRATCH" --quiet

    # 编译缓存不落在生成树里，理由同 `structure.sh`。
    export CARGO_TARGET_DIR="$GATE_ROOT/cache"
    ( cd "$dest" && cargo generate-lockfile ) || die "在展开出来的工作区里解析失败：$dest
   锁文件没有被改动。先让那棵树自己能解析，再回来跑这个。"
    ok "resolve：$dest"

    # ── local-set ───────────────────────────────────────────────────────────
    #
    # 相等，不是包含。多一个说明有第三方包混进了"无 source"这一类（不可能，除非锁文件
    # 被手改过）；少一个说明某个成员在图里和别人撞了名，于是 cargo 给它加了 source 或者
    # 改写成了消歧形式——那正是 `hooks/pre.rhai` 拒绝的那类名字，只不过这次撞的是我们
    # 自己挑的一次性名字。
    local want got
    want="$(for m in $LOCK_MEMBERS; do printf '%s-%s\n' "$LOCK_SCRATCH" "$m"; done | sort)"
    got="$(lock_local_packages "$dest/Cargo.lock")"
    if [ "$want" != "$got" ]; then
        die "local-set：锁里的本地包集合不是那六个成员。
   期望：$(printf '%s' "$want" | tr '\n' ' ')
   实际：$(printf '%s' "$got" | tr '\n' ' ')
   多出来的：锁文件被手改过，或者有第三方包没有 source 行。
   少掉的：一次性前缀 \`$LOCK_SCRATCH\` 和依赖图里某个包撞了名。换一个。"
    fi
    ok "local-set：六个成员逐个相等"

    # ── denormalize ─────────────────────────────────────────────────────────
    #
    # 写回之前先在内存里验一遍：替换之后，`$LOCK_SCRATCH` 这个串必须一次都不再出现。
    # 它是"有没有漏替换"的完整判据——一次性前缀不像真名字，不会碰巧出现在别的地方。
    local tmp_lock="$GATE_ROOT/Cargo.lock.new"
    lock_denormalize "$dest/Cargo.lock" "$LOCK_SCRATCH" > "$tmp_lock"

    local leaked
    leaked="$(grep -nH -- "$LOCK_SCRATCH" "$tmp_lock" || true)"
    if [ -n "$leaked" ]; then
        die "denormalize：替换之后还剩下一次性前缀的痕迹。
$(printf '%s\n' "$leaked" | sed 's/^/   /')
   说明成员名出现在了一个不是「整串带引号」的位置上。模板锁文件**没有**被改动。"
    fi

    # 第三方名字一个都不许动：除去成员那六行，新旧两份的 `name = ` 集合必须相等。
    # 这是 A-31 那条缺陷的直接判据——无差别替换正是在这里露馅。
    if [ "$before_count" -gt 0 ]; then
        local old_names new_names
        old_names="$(grep '^name = ' "$TEMPLATE_ROOT/Cargo.lock" | grep -vF '{{crate_prefix}}' | sort -u)"
        new_names="$(grep '^name = ' "$tmp_lock" | grep -vF '{{crate_prefix}}' | sort -u)"
        if [ "$old_names" != "$new_names" ]; then
            info "第三方包集合发生了变化（这通常是清单改动的正当结果）："
            printf '%s\n' "$(diff <(printf '%s\n' "$old_names") <(printf '%s\n' "$new_names") || true)" \
                | sed 's/^/   /'
        fi
    fi
    ok "denormalize：占位符替换完成，无残留"

    /bin/cp "$tmp_lock" "$TEMPLATE_ROOT/Cargo.lock"
    after_count="$(grep -c '^\[\[package\]\]' "$TEMPLATE_ROOT/Cargo.lock" || true)"

    # ── round-trip ──────────────────────────────────────────────────────────
    #
    # 真正的证明：拿刚写回去的这份锁重新生成一次，让 cargo 带 `--locked` 去读它。
    # 前面那些断言验的是"替换对不对"，这一条验的是"用户拿到手能不能用"——而后者才是
    # 这个脚本存在的理由。
    safe_rm_rf "$dest"
    bash "$here/gen.sh" --name "$LOCK_SCRATCH" --quiet
    ( cd "$dest" && cargo tree --workspace --depth 0 --locked >/dev/null 2>&1 ) \
        || die "round-trip：写回去的锁文件过不了 \`--locked\`。
   模板根的 Cargo.lock **已经被改动**，而它现在是坏的。用 git 回滚它，然后看上面的替换
   报告——问题一定在成员名的替换位置上。"
    ok "round-trip：新锁通过 --locked"
    safe_rm_rf "$dest"

    say ""
    if [ "$before_count" -eq "$after_count" ]; then
        info "模板 Cargo.lock 已更新：$after_count 条 [[package]]（含六个成员），条目数不变"
    else
        info "模板 Cargo.lock 已更新：$before_count → $after_count 条 [[package]]（含六个成员）"
    fi
    gate_end
}

# 被 `source` 时只提供上面两个纯函数，不跑主流程——`scripts/tooling-test.sh` 要拿
# `lock_denormalize` 喂合成输入做负向用例，而那个用例不需要、也不该真的去生成一棵树。
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    lock_main "$@"
fi
