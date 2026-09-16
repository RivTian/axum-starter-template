#!/usr/bin/env bash
# 前置条件检查。`Makefile` 里每一个依赖外部工具的目标都先跑它。
#
# 它存在的理由是**诊断质量**，不是安全性。少了 cargo-generate 的话后面的目标当然也会红，
# 但红出来的是 `cargo-generate: command not found` 或者一段 cargo 的用法提示——读的人得
# 先猜"这是环境问题还是我改坏了"。前置检查把这个问题提前到一行明确的话上。
#
# ## 这里没有 Python
#
# 门禁的依赖面被刻意压到「Rust 工具链 + POSIX shell + git」。上一版的工程链是两个 Python
# 脚本，于是"跑一次门禁"要先回答"用哪个解释器、装没装依赖"。本模板的门禁不问这个问题。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

gate_covers "bash-version"      "bash ≥ 3.2（macOS 自带的就是 3.2）"
gate_covers "posix-tools"       "find / sed / sort / perl / git 可用"
gate_covers "rust-toolchain"    "cargo 版本与 rust-toolchain.toml 的 channel 一致"
gate_covers "rust-components"   "rustfmt 与 clippy 两个组件都在"
gate_covers "cargo-generate"    "版本落在 cargo-generate.toml 声明的兼容区间内"
gate_begin  "preflight"

# ── bash ────────────────────────────────────────────────────────────────────
#
# 下界取 3.2 而不是 4：macOS 自带的 `/bin/bash` 就是 3.2，而门禁必须能在一台没装
# homebrew bash 的 mac 上跑。代价是全部脚本要避开 `mapfile` / 关联数组 / `${x^^}`。
if [ "${BASH_VERSINFO[0]}" -lt 3 ] || { [ "${BASH_VERSINFO[0]}" -eq 3 ] && [ "${BASH_VERSINFO[1]}" -lt 2 ]; }; then
    die "bash 版本过低：${BASH_VERSION}，需要 ≥ 3.2"
fi
ok "bash ${BASH_VERSION%%(*}"

# ── POSIX 工具 ──────────────────────────────────────────────────────────────
for tool in find sed sort perl git; do
    command -v "$tool" >/dev/null 2>&1 || die "缺少 $tool。门禁的依赖面是「Rust 工具链 + POSIX shell + git」。"
done
ok "find / sed / sort / perl / git"

# ── Rust 工具链 ─────────────────────────────────────────────────────────────
command -v cargo >/dev/null 2>&1 || die "缺少 cargo。装一个 rustup：https://rustup.rs"

# 钉死的版本从 rust-toolchain.toml 里读，不在这里写第二遍——两处数字迟早会不一致。
pinned="$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$TEMPLATE_ROOT/rust-toolchain.toml")"
[ -n "$pinned" ] || die "rust-toolchain.toml 里读不到 channel"

actual="$(cargo --version | awk '{print $2}')"
if [ "$actual" != "$pinned" ]; then
    die "cargo 版本是 $actual，而 rust-toolchain.toml 钉的是 $pinned。
   门禁里有逐字节比较和 \`cargo fmt --check\`，两者都对版本敏感——不一致时红出来的
   会是一段看不懂的 diff，而不是这行话。
   修法：装上 rustup（它会自动按 rust-toolchain.toml 切换），或者把钉的版本改掉
   并**重新跑一遍完整门禁**，因为那些实测数字都是在 $pinned 下量出来的。"
fi
ok "cargo $actual（与 rust-toolchain.toml 一致）"

# 组件检查用「能不能调起来」而不是问 rustup：门禁真正需要的是这两个子命令可执行，
# 而不是 rustup 的组件清单里有它们的名字。用户用别的方式装的工具链也该能过。
cargo fmt --version    >/dev/null 2>&1 || die "rustfmt 不可用。rustup component add rustfmt"
cargo clippy --version >/dev/null 2>&1 || die "clippy 不可用。rustup component add clippy"
ok "rustfmt / clippy"

# ── cargo-generate ──────────────────────────────────────────────────────────
command -v cargo-generate >/dev/null 2>&1 \
    || die "缺少 cargo-generate。cargo install cargo-generate"

# 区间同样从 cargo-generate.toml 里读。`>=0.24, <0.25` → 下界 0.24、上界 0.25。
range="$(sed -n 's/^cargo_generate_version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$TEMPLATE_ROOT/cargo-generate.toml")"
[ -n "$range" ] || die "cargo-generate.toml 里读不到 cargo_generate_version"

cg_version="$(cargo generate --version | awk '{print $NF}')"

# 只比较 major.minor。区间是 `>=X, <Y` 的形状，patch 位不参与——这正是选兼容区间
# 而不是精确版本的意思（精确钉版要配一条审计门禁才成立，本模板没有那条）。
cg_mm="$(printf '%s\n' "$cg_version" | sed 's/^\([0-9]*\.[0-9]*\).*/\1/')"
lo="$(printf '%s\n' "$range" | sed -n 's/.*>=[[:space:]]*\([0-9]*\.[0-9]*\).*/\1/p')"
hi="$(printf '%s\n' "$range" | sed -n 's/.*<[[:space:]]*\([0-9]*\.[0-9]*\).*/\1/p')"
[ -n "$lo" ] && [ -n "$hi" ] || die "cargo_generate_version 的形状不认识：$range"

# 用 sort -V 做版本比较：`0.10` 在字典序里小于 `0.9`，而这里要的是版本序。
ver_lt() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ]; }
if ver_lt "$cg_mm" "$lo" || ! ver_lt "$cg_mm" "$hi"; then
    die "cargo-generate 版本是 $cg_version，不在声明的区间 $range 内。
   \`include\` / \`ignore\` 的语义由 cargo-generate 自己实现，跨 minor 变过一次以上——
   区间之外的版本可能让本该被删的文件进生成结果，而那是静默的。
   修法：cargo install cargo-generate --version '$range'"
fi
ok "cargo-generate $cg_version（区间 $range）"

gate_end
