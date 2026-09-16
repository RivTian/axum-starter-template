#!/usr/bin/env bash
# 走用户**真实的** `--git` 安装路径生成一次，并验两件只有这条路上才看得见的事。
#
#   scripts/verify-git.sh [--quiet]
#
# ## 为什么它不能并进 `check`
#
# `check` 里的 `gen` 走 `--path`，那是"复制目录再展开"。`--git` 是另一条：cargo-generate
# 先 clone 出一个新的 checkout，于是两件在 `--path` 下被完全绕过的事第一次参与进来——
#
#   1. **"哪些文件真的被提交了"**。`--path` 照着工作树复制，一个忘了 `git add` 的文件在
#      门禁里永远是绿的，在用户那里永远是缺的。
#   2. **换行符规则**。新 checkout 会套用本机的 `core.autocrlf`。同一个提交在两台机器上
#      可以有不同的字节，而这个模板的门禁比的正是字节。
#
# 第 2 件是 `.gitattributes` 里那条 `* -text` 存在的**全部**理由，而它在 `--path` 那条路上
# 一点效果都没有——也就是说，不跑这一趟，那个文件是否起作用没有任何证据。
#
# 它没并进 `check`，是因为它要连着生成三棵树（正向、敌意配置、负向对照），而 `check` 已经
# 把名字矩阵的四趟背在身上了。它是 CI 里单独的一格。
#
# ## 敌意配置怎么注进去
#
# `GIT_CONFIG_GLOBAL` 指向一份写着 `autocrlf = true` 的配置。实测（cargo-generate 0.24.0）：
# 这条**有效**，克隆出来的文本文件会变成 CRLF；而命令行上的 `--gitconfig` 对 checkout 的
# 换行转换**无效**（它管的是别的事）。两条都试过，留着这句是为了下次不用再试一遍。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HERE="$(dirname "${BASH_SOURCE[0]}")"
GEN_NAME="gitsvc"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,5p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "verify-git.sh: 不认识的参数：$1" ;;
    esac
done

gate_covers "git-path"      "经 --git 生成一次，并当场跑完生成即审计五项"
gate_covers "absent-via-git" "结构检查在 --git 树上全过：模板自身的东西一件都没被带进来"
gate_covers "crlf-neutral"  "克隆方 core.autocrlf=true 时，生成树与常规那趟逐字节相同"
gate_covers "crlf-negative" "负向对照：拿掉 .gitattributes，同一趟必须出现 CRLF"
gate_begin  "verify-git"

gate_workspace_init
# 整趟只锁一次；下面的 gen / structure 是本进程的子进程，见 lib.sh::gate_lock 的可重入说明。
gate_lock

W="$GATE_ROOT/verify-git"
safe_rm_rf "$W"
mkdir -p -- "$W"

# ── git-path ────────────────────────────────────────────────────────────────
#
# 生成本身连同那五项审计一起委托给 `gen.sh --git`：审计的判据只该有一份定义，在这里再抄
# 一遍的结果是两份会各自漂移。
bash "$HERE/gen.sh" --git --name "$GEN_NAME" --quiet
GEN_DEST="$GATE_ROOT/gen/$GEN_NAME"
[ -d "$GEN_DEST" ] || die "verify-git：gen.sh --git 说它成功了，但 $GEN_DEST 不在"
ok "git-path：经 --git 生成于 $GEN_DEST，审计五项随之跑完"

# ── crlf-neutral ────────────────────────────────────────────────────────────
#
# 拿同一个快照仓库、同一个名字，在敌意配置下再生成一次，两棵树必须逐字节相同。
#
# 这里直接用 `gen.sh --git` 留下的快照仓库，而不是自己再 tar 一份：快照怎么做（排除哪些
# 目录、提交用什么身份）只该有一份实现。代价是依赖了它的一个内部路径，所以下面那条断言
# 指名道姓地说出这层依赖——路径变了的时候，红的是这句话而不是一个莫名其妙的 clone 失败。
SNAPSHOT="$GATE_ROOT/gitsrc"
[ -d "$SNAPSHOT/.git" ] || die "verify-git：找不到 gen.sh --git 留下的快照仓库 $SNAPSHOT。
   这条门禁复用它来做敌意配置那一趟。gen.sh 里那个路径改过的话，把这里一起改。"

cat > "$W/gitconfig" <<'EOF'
[core]
	autocrlf = true
EOF

mkdir -p -- "$W/hostile"
hostile_log="$W/hostile.log"
( cd "$W/hostile" && GIT_CONFIG_GLOBAL="$W/gitconfig" \
    cargo generate --git "$SNAPSHOT" --name "$GEN_NAME" --force ) > "$hostile_log" 2>&1 \
    || { cat "$hostile_log" >&2; die "verify-git：敌意配置那一趟生成失败，日志见 $hostile_log"; }

# 排除 `.git`：两趟各自被 cargo-generate `git init` 过，里面的对象名和时间戳必然不同，
# 而那与换行符无关。比的是工作树。
if ! diff -r -x .git -- "$GEN_DEST" "$W/hostile/$GEN_NAME" > "$W/crlf.diff" 2>&1; then
    head -40 "$W/crlf.diff" | sed 's/^/   /'
    die "verify-git：克隆方 core.autocrlf=true 时，生成树与常规那趟**不再逐字节相同**（差异见上，完整见 $W/crlf.diff）。
   这正是 .gitattributes 里 \`* -text\` 要挡住的事：没挡住的话，同一个提交在不同机器上
   会展开出不同的字节，而这个模板的结构门禁比的就是字节——用户那边红，你这边永远绿。"
fi
ok "crlf-neutral：敌意配置下两棵树逐字节相同"

# ── crlf-negative ───────────────────────────────────────────────────────────
#
# 上面那条绿了，有两种可能：`* -text` 起了作用，或者这台机器 / 这个 git 版本根本就不做
# 换行转换。两者在输出上一模一样。所以再跑一趟拿掉 `.gitattributes` 的对照——它**必须**
# 出现 CRLF，否则上面那条绿说明不了任何事。
#
# 快照仓库先整个复制一份再改：直接在 `$GATE_ROOT/gitsrc` 上提交会把 `gen.sh` 下一次
# `--git` 的输入弄脏，而那种脏是留在磁盘上跨运行传播的。
NOATTR="$W/gitsrc-noattr"
( cd "$SNAPSHOT" && tar -cf - . ) | ( mkdir -p -- "$NOATTR" && cd "$NOATTR" && tar -xf - ) \
    || die "verify-git：复制快照仓库失败"
(
    cd "$NOATTR" \
        && git rm -q --cached .gitattributes \
        && rm -f .gitattributes \
        && git -c user.email=gate@invalid -c user.name=gate \
               commit -q -m "negative control: drop .gitattributes"
) > "$W/noattr.log" 2>&1 \
    || { cat "$W/noattr.log" >&2; die "verify-git：负向对照仓库准备失败，日志见 $W/noattr.log"; }

mkdir -p -- "$W/noattr-out"
( cd "$W/noattr-out" && GIT_CONFIG_GLOBAL="$W/gitconfig" \
    cargo generate --git "$NOATTR" --name "$GEN_NAME" --force ) > "$W/noattr-gen.log" 2>&1 \
    || { cat "$W/noattr-gen.log" >&2; die "verify-git：负向对照那一趟生成失败，日志见 $W/noattr-gen.log"; }

# 找一个真的含 CRLF 的文件。逐文件看，是为了让失败信息说得出"哪个文件本该变脏"。
crlf_hits="$(list_text_files "$W/noattr-out/$GEN_NAME" \
    | perl -0 -ne 'chomp; open(my $fh,"<",$_) or next; binmode $fh; local $/; my $c=<$fh>;
                   print "$_\n" if index($c,"\r\n") >= 0' | wc -l | tr -d ' ')"
[ "$crlf_hits" -gt 0 ] || die "verify-git：负向对照**没有**出现 CRLF。
   拿掉 .gitattributes 之后本该出现换行转换，没出现说明这台机器上这条实验根本不成立
   （git 版本、克隆实现或 core.autocrlf 的语义变了）。
   后果：上面 crlf-neutral 那条绿证明不了 \`* -text\` 起了作用——它可能只是在验一个
   本来就不会发生的转换。不要把它当成证据。"
ok "crlf-negative：拿掉 .gitattributes 后 $crlf_hits 个文件变成 CRLF，实验成立"

# ── absent-via-git ──────────────────────────────────────────────────────────
#
# 放在最后：结构检查会读生成树，而上面那两条比的是它的字节，顺序反过来就得先证明
# `structure.sh` 不写它——多一条需要维护的前提。
#
# 这一趟是 §15.2 里「`--git` 模式下 `ignore` 与 `hooks/` 的时序无证据」那条的落点：
# `structure.sh::absent` 把两份名单（`cargo-generate.toml` 的 `ignore` 与
# `post.rhai::assert_absent`）交叉比对，再拿生成树核对一遍。之前它只在 `--path` 树上跑过。
bash "$HERE/structure.sh" --name "$GEN_NAME" --quiet
ok "absent-via-git：结构检查在 --git 树上全过"

safe_rm_rf "$W"
gate_end
