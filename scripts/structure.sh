#!/usr/bin/env bash
# 生成结果的结构门禁。跑在 `scripts/gen.sh` 之后，读的是它留在工作区里的那棵生成树。
#
#   scripts/structure.sh [--name NAME] [--quiet]
#
# ## 它验的是"图"，不是"文本"
#
# 这里每一条依赖断言都从 **cargo 解析之后的图**上读，没有一条去 grep `Cargo.toml`。
# 差别不是风格问题，是两件事：
#
#   1. 清单文本回答不了"最终生效的是什么"。`[workspace.dependencies]` 的继承、
#      feature 的并集、可选依赖被谁打开——这些只在解析结果里才成立。一个 feature 可以
#      在三个地方各写一半，文本视角看每一处都是干净的。
#   2. 清单文本分不清 `[dependencies]` 和 `[dev-dependencies]`。本工作区里
#      `rt-multi-thread` 在 `core` / `storage` / `api` 三份清单里都出现过——全是
#      dev-dependency，全是正当的。一条朴素的 grep 会把这三处判红，然后人会去放宽判据，
#      最后放宽到连真违例也拦不住。
#
# 读图的方式是 `cargo tree` 而不是 `cargo metadata`：后者的输出是 JSON，而门禁的依赖面
# 被压到「Rust 工具链 + POSIX shell + git」，里面没有 `jq`。`cargo tree --prefix depth`
# 把树的层级变成行首的十进制数字，于是解析它只需要 `sed`。
#
# ## 它**不**验什么
#
# 生成结果自己的 `testkit/tests/discipline.rs` 里有二十条结构纪律，跑在生成结果的
# `make check` 里。那二十条从**源码与清单文本**的角度看结构（公共出口清单比对、
# `select!` 取消优先、`allow` 的作用域……）。本脚本刻意不重复它们：同一条纪律写两遍，
# 改的时候只会改一边。
#
# 唯一有意重叠的是「单一可执行入口」，理由写在那一段上。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

GEN_NAME="demo-svc"
while [ $# -gt 0 ]; do
    case "$1" in
        --name)  GEN_NAME="$2"; shift 2 ;;
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "不认识的参数：$1" ;;
    esac
done

GEN_DEST="$GATE_ROOT/gen/$GEN_NAME"

# 名单比对用 C 排序。`sort` 和 `comm` 必须站在同一套排序规则上，而 UTF-8 locale 下
# 前者忽略标点、后者按字节比——`.DS_Store` 这种带前导点的条目会让两者得出不同的结论，
# 而那个不一致的表现形式是一条凭空多出来的差异行。
export LC_ALL=C

gate_covers "no-symlink"     "模板树里没有符号链接（C-121）"
gate_covers "name-collision" "pre.rhai 拒绝的前缀清单 == 锁文件反推出来的撞名集合"
gate_covers "absent"         "模板自身的东西一件都没进来；两份名单交叉比对"
gate_covers "byte-identical" "非渲染面的文件在生成前后逐字节相同"
gate_covers "no-junk"        "生成树里没有 Finder / 编辑器留下的垃圾"
gate_covers "single-bin"     "整个工作区只有一个可执行入口"
gate_covers "adjacency"      "本地依赖边与分层邻接表逐格相等（读解析后的图）"
gate_covers "third-party"    "第三方 crate 的所在层受限（D3）"
gate_covers "sqlx-features"  "sqlx 与 libsqlite3-sys 的已解析 feature 集逐包断言（D39）"
gate_begin  "structure"

gate_workspace_init
gate_lock

[ -d "$GEN_DEST" ] || die "生成树不存在：$GEN_DEST
   本脚本读的是 gen 留下的产物，自己不生成。先跑：
     scripts/gen.sh --name $GEN_NAME"

# 六个成员的层名。全脚本只在这里写一次。
MEMBERS="core storage worker api app testkit"

# 把一列名字压成一行，供报错和信息行使用。
joined() { tr '\n' ' ' | sed 's/ $//'; }

# 包名前缀从生成树里**读出来**，不从 `$GEN_NAME` 推。两者在默认名字下相同，但
# `hooks/pre.rhai` 对带连字符前缀、带大写、过短的名字会做推导和修正（名字矩阵里
# 专门有这几组），推一遍等于把那段逻辑在门禁里复制一份——然后两份会不一致。
PREFIX="$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\(.*\)-core"[[:space:]]*$/\1/p' \
    "$GEN_DEST/core/Cargo.toml" | head -1)"
[ -n "$PREFIX" ] || die "从 $GEN_DEST/core/Cargo.toml 里读不出包名前缀"

# 编译缓存不落在生成树里：生成树要参与逐字节比较和垃圾扫描，多一个 `target/` 会让这两条
# 检查各自长出一条例外。缓存跨门禁复用，也顺带让 matrix 的四趟不必各编一遍。
export CARGO_TARGET_DIR="$GATE_ROOT/cache"

WORK="$GATE_ROOT/structure-$GEN_NAME"
safe_rm_rf "$WORK"
mkdir -p -- "$WORK"

# ── no-symlink（C-121）────────────────────────────────────────────────────────
#
# 扫的是**模板树**，不是生成树。理由是 cargo-generate 0.24.0 的拷贝实现
# （`copy_files_recursively`）对符号链接是**静默跳过**：它只 warn 一句，然后继续，
# 退出码仍然是 0。于是"模板里有个链接"这件事的表现形式是"生成结果里少一个文件"——
# 而少的那个文件如果不在逐字节比较的覆盖面内（比如它本该在一个 `ignore` 掉的目录里
# 被引用），就没有任何东西会红。唯一能稳住这件事的位置是模板侧：不许有链接。
links="$(find "$TEMPLATE_ROOT" \( -name .git -o -name target \) -prune -o -type l -print)"
if [ -n "$links" ]; then
    die "模板树里有符号链接：
$links
   cargo-generate 拷贝时会静默跳过它们（只 warn，退出码仍是 0），
   于是生成结果里会少文件而门禁全绿。把链接换成真文件。"
fi
ok "no-symlink（模板树）"

# ── name-collision ───────────────────────────────────────────────────────────
#
# `hooks/pre.rhai` 里有一份"不许拿来当项目名"的前缀清单。它不是审美判断，是一条实测出来
# 的硬约束：六个成员叫 `<prefix>-{core,storage,worker,api,app,testkit}`，只要其中任何
# 一个名字在已锁定的依赖图里已经存在，随模板发布的那份 `Cargo.lock` 就不再匹配——
# cargo 在两个同名包之间会把依赖引用从裸名改写成 `"name version"` 的消歧形式，而发布出去
# 的那份锁文件是在一个不撞名的前缀下生成的。生成结果的三条门禁全带 `--locked`，于是用户
# 敲的第一条命令就红，报的是锁文件的事。
#
# 这份清单**必须跟着依赖集走**：加一条新依赖就可能多出一个撞名前缀，而那一天没有任何东西
# 会提醒谁回来改 `pre.rhai`。所以在这里从锁文件重算一遍再比对——与 `absent` 那两份名单
# 同一种做法，理由也同一条：靠人记是记不住的。
collision_from_lock() {
    local suf
    for suf in $MEMBERS; do
        sed -n "s/^name = \"\(.*\)-$suf\"\$/\1/p" "$TEMPLATE_ROOT/Cargo.lock"
    done | grep -vF '{{crate_prefix}}' | sort -u
}
collision_from_hook() {
    sed -n 's/^[[:space:]]*let reserved = \[\(.*\)\];[[:space:]]*$/\1/p' "$TEMPLATE_ROOT/hooks/pre.rhai" \
        | tr ',' '\n' | sed -n 's/.*"\([^"]*\)".*/\1/p' | sort -u
}

collision_from_lock > "$WORK/collide-lock.txt"
collision_from_hook > "$WORK/collide-hook.txt"
[ -s "$WORK/collide-lock.txt" ] || die "从 Cargo.lock 里一个撞名前缀都没算出来——解析坏了
   （锁文件里至少有 axum-core / futures-core 这一类，算出来是空的说明 sed 没匹配上）"
[ -s "$WORK/collide-hook.txt" ] || die "从 hooks/pre.rhai 里读不出 reserved 清单——解析坏了"

if ! diff -q "$WORK/collide-lock.txt" "$WORK/collide-hook.txt" >/dev/null; then
    die "撞名前缀清单对不上（左：Cargo.lock 反推；右：pre.rhai 的 reserved）：
$(diff "$WORK/collide-lock.txt" "$WORK/collide-hook.txt" | sed 's/^/   /')
   左边多出来的：有人用这个名字建项目，第一条命令就会红在 --locked 上，而报错不提名字。
   右边多出来的：一个没有必要的拒绝，用户被挡在一个其实能用的名字外面。
   两边都只有一个修法：改 pre.rhai 的 reserved 清单，让它等于左边。"
fi
ok "name-collision：$(joined < "$WORK/collide-hook.txt")"

# ── absent ───────────────────────────────────────────────────────────────────
#
# 两份名单的交叉比对：`cargo-generate.toml` 的 `ignore` 负责删，`hooks/post.rhai` 的
# `assert_absent` 负责在用户侧复核删干净了没有。两份名单必须同时改——而"同时改"这件事
# 靠人记是记不住的，所以在这里比一次。
#
# 差集不是空的，差的正好是 `Makefile` 和 `README.md` 两条：它们被 `ignore` 删掉之后，
# 又由 `post.rhai::promote` 从 `Makefile.project` / `README.project.md` 改名补回来。
# 对这两条断言"不存在"会必然失败。这条规则写成等式，而不是写成"大致一致"：
#
#     assert_absent 集合  ==  ignore 集合 − { Makefile, README.md }
ignore_list() {
    sed -n '/^ignore = \[/,/^\]/p' "$TEMPLATE_ROOT/cargo-generate.toml" \
        | sed -n 's/^[[:space:]]*"\([^"]*\)",.*/\1/p'
}
absent_list() {
    sed -n 's/^assert_absent("\([^"]*\)".*/\1/p' "$TEMPLATE_ROOT/hooks/post.rhai"
}

ignore_list | sort -u > "$WORK/ignore.txt"
absent_list | sort -u > "$WORK/absent.txt"
printf '%s\n' "Makefile" "README.md" | sort > "$WORK/promoted.txt"
comm -23 "$WORK/ignore.txt" "$WORK/promoted.txt" > "$WORK/expected-absent.txt"

[ -s "$WORK/ignore.txt" ] || die "从 cargo-generate.toml 里一条 ignore 都没读出来——解析坏了"
[ -s "$WORK/absent.txt" ] || die "从 hooks/post.rhai 里一条 assert_absent 都没读出来——解析坏了"

if ! diff -q "$WORK/expected-absent.txt" "$WORK/absent.txt" >/dev/null; then
    die "两份名单对不上（左：ignore − {Makefile,README.md}；右：post.rhai 的 assert_absent）：
$(diff "$WORK/expected-absent.txt" "$WORK/absent.txt" | sed 's/^/   /')
   两份名单必须同时改：ignore 负责删，assert_absent 负责在用户侧复核删干净了没有。"
fi
ok "absent：两份名单相等（$(wc -l < "$WORK/absent.txt" | tr -d ' ') 条）"

while IFS= read -r p; do
    if [ -e "$GEN_DEST/$p" ]; then
        die "模板自身的 \`$p\` 进了生成结果：$GEN_DEST/$p
   \`ignore\` 收的是字面路径、拼错静默无效（C-113），这正是它会失效的样子。"
    fi
done < "$WORK/absent.txt"

# `promote` 的两侧都验：改名前的名字必须消失，改名后的必须在。只验后者的话，
# 一个"拷贝而不是改名"的实现会让生成结果里同时躺着 `Makefile` 和 `Makefile.project`，
# 而两者内容相同、编译也不受影响——那是一种不会被任何其它检查发现的脏。
for f in Makefile README.md; do
    [ -f "$GEN_DEST/$f" ] || die "生成结果里没有 $f——post.rhai 的 promote 没生效"
done
for f in Makefile.project README.project.md; do
    if [ -e "$GEN_DEST/$f" ]; then
        die "生成结果里还留着 $f——promote 是拷贝不是改名？"
    fi
done

# cargo-generate 无条件删的那三个，加上 post.rhai 自己收走的 `hooks/`。
# 它们不在任何一份名单里（写进 `ignore` 反而会让 post 钩子在轮到它之前就被删掉），
# 所以只有这里能验。
for f in cargo-generate.toml .genignore .cargo-ok hooks; do
    if [ -e "$GEN_DEST/$f" ]; then
        die "生成结果里残留 $f——它本该被 cargo-generate / post.rhai 删掉"
    fi
done
ok "absent：$(wc -l < "$WORK/absent.txt" | tr -d ' ') 条名单项 + 改名两侧 + 工具自删四项"

# ── byte-identical ───────────────────────────────────────────────────────────
#
# 渲染面之外的文件必须在生成前后逐字节相同。这是 DP1 收益的执行点：`.rs` 一个都不进
# 渲染面，于是 rustfmt 的结果与项目名无关。少了这条断言，某天有人往 `include` 里加一条
# `**/*.rs`，所有测试照样绿——因为生成结果仍然能编译，只是从此项目名混进了源码。
#
# 遍历方向是从**生成树**往模板树查，不是反过来。反向遍历要先把 `ignore` 的效果重算一遍
# 才知道哪些文件"本来就不该在"，等于把 cargo-generate 的删除逻辑抄第二遍。正向遍历不需要
# 知道那些：凡是进了生成结果的，要么在渲染面里，要么必须与模板里的那一份相同。
is_rendered() {
    case "$1" in
        Cargo.toml|*/Cargo.toml) return 0 ;;  # 包名、`[lib] name`、兄弟 crate 的引用
        Cargo.lock)              return 0 ;;  # 锁文件里有本地包的真名
        README.md|*/README.md)   return 0 ;;  # 根 README 由 README.project.md 改名而来
        Makefile)                return 0 ;;  # 同上，来自 Makefile.project
        *)                       return 1 ;;
    esac
}

same=0; rendered=0
while IFS= read -r -d '' f; do
    rel="${f#"$GEN_DEST"/}"
    if is_rendered "$rel"; then
        rendered=$((rendered + 1))
        continue
    fi
    src="$TEMPLATE_ROOT/$rel"
    [ -f "$src" ] || die "生成结果里的 \`$rel\` 在模板树里没有对应文件。
   它既不在渲染面里，也不是模板里的文件——那它是从哪来的？"
    cmp -s -- "$src" "$f" || die "\`$rel\` 在生成前后不是逐字节相同的。
   它不在 cargo-generate.toml 的 include 白名单里，本该被逐字节拷贝。
   如果这是有意的，改的是 include 白名单，不是这条断言——但先想清楚代价：
   进了渲染面的文件，它的内容从此与项目名有关。"
    same=$((same + 1))
done < <(find "$GEN_DEST" \( -name .git -o -name target \) -prune -o -type f -print0)

# 下界而不是精确值（C-131）：精确值会让每加一个文件都红一次，而那种红没有信息量。
# 下界只回答一个问题——这条扫描还认得出这棵树吗。
[ "$same" -ge 40 ] || die "逐字节比较只覆盖到 $same 个文件，低于下界 40。
   这几乎总是意味着遍历或路径拼接坏了，而不是模板真的只剩这么点文件。"
ok "byte-identical：$same 个文件逐字节相同，$rendered 个在渲染面里（跳过）"

# ── no-junk（C-139）──────────────────────────────────────────────────────────
#
# 扫的是**生成树**。仓库根上的那几个由 `ignore` + `assert_absent` 管（那是它们出现的
# 绝大多数位置），但 `ignore` 收的是字面路径，管不了 `core/src/.DS_Store`。而
# cargo-generate 不看模板自己的 `.gitignore`（C-138），所以 `.gitignore` 挡得住提交、
# 挡不住拷贝——子目录里的垃圾会原样进用户的项目。零容忍：这里扫出来的每一条都是泄漏。
junk="$(find "$GEN_DEST" \( -name .git -o -name target \) -prune -o \
    \( -name '.DS_Store' -o -name 'Thumbs.db' -o -name 'desktop.ini' \
       -o -name '.idea' -o -name '.vscode' -o -name '*.swp' -o -name '*~' -o -name '.#*' \) \
    -print)"
if [ -n "$junk" ]; then
    die "生成结果里有编辑器 / 文件管理器留下的垃圾：
$(printf '%s\n' "$junk" | sed "s|^$GEN_DEST/|   |")
   它们来自模板目录。cargo-generate 不看模板的 .gitignore（C-138），
   唯一的删除机制是 cargo-generate.toml 的 ignore，而它只收字面路径。
   修法：先把模板树清干净；如果是仓库根上的，往 ignore 和 assert_absent 各加一条。"
fi
ok "no-junk（生成树递归）"

# ── single-bin ───────────────────────────────────────────────────────────────
#
# 这一条与生成结果自己的 `the_workspace_has_exactly_one_executable_entry_point` 重叠，
# 是**有意**的，理由是顺序和诊断质量：本脚本跑在 `project-check` 之前，而多一个 bin
# 之后 cargo 报的是 `cargo run` 无法确定运行哪个目标——读的人得先想明白那和结构纪律
# 是同一件事。这里给的是一句话。
#
# 两个角度各验一次，因为它们各自漏掉对方拦得住的东西：
#
#   · 文件系统：只有一个 `src/main.rs`，且在 `app`。
#   · 清单：`[[bin]]` 只在 `app` 出现、且只出现一次。`[[bin]]` 能把任意一个 `.rs` 变成
#     可执行入口，只数 `main.rs` 对它视而不见。
#
# `app` **有** `[[bin]]` 是刻意的，不是违例：包名是 `<项目名>-app`，而用户敲
# `cargo run` 之后想拿到的可执行文件叫 `<项目名>`。少了这一段，产物名字会带上分层的
# 后缀——那是实现细节，不该出现在用户面前。所以这里连它的 `name =` 一起验。
mains="$(find "$GEN_DEST" \( -name .git -o -name target \) -prune -o -path '*/src/main.rs' -print | sort)"
main_count="$(printf '%s\n' "$mains" | grep -c . || true)"
[ "$main_count" -eq 1 ] || die "工作区里有 $main_count 个 src/main.rs，应当只有一个：
$(printf '%s\n' "$mains" | sed "s|^$GEN_DEST/|   |")"
[ "$mains" = "$GEN_DEST/app/src/main.rs" ] || die "唯一的可执行入口不在 app 层：$mains"

for m in $MEMBERS; do
    n="$(grep -c '^\[\[bin\]\]' "$GEN_DEST/$m/Cargo.toml" || true)"
    case "$m:$n" in
        app:1) ;;
        app:*) die "app/Cargo.toml 里有 $n 段 [[bin]]，应当只有一段。" ;;
        *:0)   ;;
        *)     die "$m/Cargo.toml 里声明了 [[bin]]。可执行入口只能在 app 层。" ;;
    esac
done

bin_name="$(sed -n '/^\[\[bin\]\]/,/^\[/p' "$GEN_DEST/app/Cargo.toml" \
    | sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
[ "$bin_name" = "$PREFIX" ] || die "可执行文件叫 \`$bin_name\`，而项目叫 \`$PREFIX\`。
   包名带 \`-app\` 后缀是分层的实现细节，不该出现在用户拿到的产物名字上。"
ok "single-bin：app/src/main.rs → 可执行文件 \`$bin_name\`，其余五层无 [[bin]]"

# ── adjacency ────────────────────────────────────────────────────────────────
#
# §3.3 的邻接表，逐格相等。
#
#   from \ to │ core storage worker api  app  testkit
#   core      │  —    ✗      ✗     ✗    ✗     ✗
#   storage   │  ✔    —      ✗     ✗    ✗     ✗
#   worker    │  ✔    ✗      —     ✗    ✗     ✗
#   api       │  ✔    ✗      ✗     —    ✗     ✗
#   app       │  ✔    ✔      ✔     ✔    —     ✗
#   testkit   │  ✔    ✗      ✗     ✗    ✗     —
#
# dev 依赖是唯一允许的额外边，方向只能是 `* → testkit`。它用**子集**而不是相等来验：
# 一个成员需不需要测试夹具是它自己的事（当前 `core` 就没有这条边），而"夹具只能被
# dev 依赖"才是纪律。相等会把"给 core 的测试加一条 testkit 依赖"这种正当改动判红。
#
# 反方向的那半条纪律——`testkit` 不许成为任何人的 normal 依赖——由上面的相等比较覆盖：
# 六个期望集合里没有一个含 `testkit`。
# 「本地成员」的判据是**逐个列出的六个名字**，不是"以 `<前缀>-` 开头"。
#
# 后者看起来等价，实际上在名字矩阵的"与 crates.io 撞名"那一组下当场就错：前缀取 `tokio`
# 时 `tokio-util`（一个正当的第三方依赖）会被认成一个叫 `util` 的本地成员，于是 `storage`
# 行凭空多出一条边。这条假红不是理论上的——它就是那一组第一次跑出来的结果。
#
# 名单从 `$MEMBERS` 拼出来，不在这里再抄一遍：只有一个地方写着"成员有哪六个"。
MEMBER_RE="$(printf '%s' "$MEMBERS" | tr ' ' '|')"

local_edges() {  # $1 = 层名，$2 = 边的种类（normal | dev）
    ( cd "$GEN_DEST" && cargo tree -e "$2" -p "$PREFIX-$1" --depth 1 --prefix depth --locked 2>/dev/null ) \
        | sed -E -n "s/^1$PREFIX-($MEMBER_RE)[[:space:]].*/\1/p" | sort -u
}

expected_normal() {
    case "$1" in
        core)    printf '' ;;
        storage) printf 'core' ;;
        worker)  printf 'core' ;;
        api)     printf 'core' ;;
        app)     printf 'api core storage worker' ;;
        testkit) printf 'core' ;;
        *)       die "expected_normal: 不认识的层名 $1" ;;
    esac
}

# 直接 normal 依赖（含第三方）缓存下来，third-party 那一段还要用。
for m in $MEMBERS; do
    ( cd "$GEN_DEST" && cargo tree -e normal -p "$PREFIX-$m" --depth 1 --prefix depth --locked ) \
        | sed -n 's/^1\([^ ]*\)[[:space:]].*/\1/p' | sort -u > "$WORK/normal.$m" \
        || die "cargo tree 读不出 $PREFIX-$m 的依赖——生成树能解析吗？"
    [ -s "$WORK/normal.$m" ] || [ "$m" = core ] \
        || die "$m 的直接依赖读出来是空的——解析坏了（core 之外没有零依赖的层）"
done

for m in $MEMBERS; do
    actual="$(local_edges "$m" normal | joined)"
    want="$(expected_normal "$m")"
    [ "$actual" = "$want" ] || die "邻接表第 \`$m\` 行对不上：
   期望：${want:-（无本地依赖）}
   实际：${actual:-（无本地依赖）}
   这张表是分层的定义本身。改它之前先回答：新增的这条边让哪个层能看见它本不该看见的东西。"

    dev="$(local_edges "$m" dev | joined)"
    for d in $dev; do
        [ "$d" = testkit ] || die "\`$m\` 有一条指向 \`$d\` 的 dev 依赖。
   dev 依赖是唯一允许的额外边，但方向只能是 * → testkit。
   一条 $m → $d 的 dev 边意味着 $m 的测试在拿 $d 的内部当夹具用。"
    done
    info "$m：normal = [${actual:-}]，dev = [${dev:-}]"
done
ok "adjacency：六行逐格相等，dev 边全部指向 testkit"

# ── third-party（D3）────────────────────────────────────────────────────────
#
# 第三方 crate 的**所在层**受限。§5.6 / §5.7 的纪律（门面不泄漏、遥测初始化只在组合根）
# 只有落到"谁能依赖它"上才有强制力——一个类型没出现在签名里，不等于那个 crate 没被引进来。
#
# 每一条的允许集写成集合而不是单个层名，因为有一条确实是两个层，而且理由要写下来：
#
#   tracing-subscriber → { app, testkit }
#     设计里写的是"只在 app"。实际的允许集多一个 testkit，采纳的是结论而不是字面：
#     纪律管的是**生产图**——组合根之外没有谁能装全局 subscriber。而 testkit 从构造上
#     就不在生产图里（它是所有人的 dev 依赖、不是任何人的 normal 依赖，上一段已逐格验过），
#     它里面的 subscriber 是测试用的私有捕获器，进不了任何二进制。
#     把 testkit 从允许集里去掉的代价是：日志断言要么消失，要么在四个 crate 的测试模块里
#     各抄一份——而后者正是 testkit 存在的理由。
declare_owner() {  # $1 = crate 名，其余 = 允许出现的层
    local crate="$1"; shift
    local want; want="$(printf '%s\n' "$@" | sort | joined)"
    local got=""
    local m
    for m in $MEMBERS; do
        if grep -qxF -- "$crate" "$WORK/normal.$m"; then got="$got $m"; fi
    done
    got="${got# }"
    [ "$got" = "$want" ] || die "\`$crate\` 的所在层不对：
   允许：$want
   实际：${got:-（没有任何层依赖它——它还在这个工作区里吗？）}"
    info "$crate → [$got]"
}

declare_owner sqlx               storage
declare_owner axum               api
declare_owner tower-http         api
declare_owner tracing-subscriber app testkit

# `rt-multi-thread` 是 tokio 的一个 feature，不是一个包，所以它没法用上面那套。
#
# 判据是"哪些成员的**去掉 dev 边之后**的 feature 图里含这个节点"。去掉 dev 边是关键：
# `core` / `storage` / `api` 的 dev-dependencies 里都有它（测试要起多线程运行时，正当），
# 而一条朴素的文本 grep 会把这三处判红。
#
# 用正向图（`-p <成员>`）而不是反向图（`--invert tokio`）：反向图里 `(*)` 会把已经打印过
# 的子树折叠掉，于是"谁最终打开了它"可能落在折叠掉的那一段里；正向图只需要回答"这个节点
# 在不在我的图里"，而节点本身无论如何都会被打印至少一次。
rt_owners=""
for m in $MEMBERS; do
    if ( cd "$GEN_DEST" && cargo tree -e no-dev,features -p "$PREFIX-$m" --prefix depth --locked ) \
        | grep -qF 'tokio feature "rt-multi-thread"'; then
        rt_owners="$rt_owners $m"
    fi
done
rt_owners="${rt_owners# }"
[ "$rt_owners" = "app" ] || die "打开 tokio/rt-multi-thread 的层是 [$rt_owners]，应当只有 app。
   选运行时的形状是组合根的决定。一个库层把多线程运行时打开，等于替它的每一个使用者
   做了这个决定，而使用者连这件事发生过都不知道。"
info "tokio/rt-multi-thread → [$rt_owners]"
ok "third-party：五条定位断言全部相等"

# ── sqlx-features（D39）─────────────────────────────────────────────────────
#
# 判的是 **cargo 解析之后的 feature 集**，逐包断言，不读 `Cargo.lock` 的包列表。
#
# 后者是个很容易犯的错：`Cargo.lock` 里没有 `sqlx-postgres` 这一行，看起来就等于
# "没有引进 postgres"。但那是**结果**，不是**约束**——feature 的并集由整个图决定，
# 今天没有不等于加一个依赖之后不会有，而那一天 `Cargo.lock` 会静默地多一行。
# 断言在 feature 集上，改动才会在加进来的那一刻红。
resolved_features() {  # $1 = 包名
    ( cd "$GEN_DEST" && cargo tree -e features --invert "$1" --prefix depth --locked 2>/dev/null ) \
        | sed -n "s/^1$1 feature \"\([^\"]*\)\".*/\1/p" | sort -u
}

assert_features() {  # $1 = 包名，$2 = 必须有（空格分隔），$3 = 必须没有（可为空）
    local pkg="$1" must="$2" forbid="$3" f
    resolved_features "$pkg" > "$WORK/feat.$pkg"
    [ -s "$WORK/feat.$pkg" ] || die "读不出 $pkg 的已解析 feature 集——它在图里吗？"
    for f in $must; do
        grep -qxF -- "$f" "$WORK/feat.$pkg" \
            || die "$pkg 缺 feature \`$f\`。已解析的是：$(joined < "$WORK/feat.$pkg")"
    done
    for f in $forbid; do
        if grep -qxF -- "$f" "$WORK/feat.$pkg"; then
            die "$pkg 打开了 feature \`$f\`，而它在禁用清单里。
   这条 feature 会把一整个数据库驱动拉进编译闭包，而本骨架只声明支持一个后端。"
        fi
    done
    info "$pkg：$(joined < "$WORK/feat.$pkg")"
}

# `sqlite-bundled`：驱动自带 SQLite 源码，不去链接系统库——生成结果的依赖面只有 Rust
# 工具链，这一条是它的前提。`any` 是那个最容易被顺手打开的：它会把全部驱动一起编。
assert_features sqlx "sqlite-bundled runtime-tokio migrate macros" "any postgres mysql"
# 上一条的另一半：`sqlite-bundled` 生效的形式就是 `libsqlite3-sys` 的 `bundled`。
# 只验前者的话，一个把 sqlx 换成别的包装层的改动会让前者仍然成立而后者悄悄消失。
assert_features libsqlite3-sys "bundled" ""
ok "sqlx-features：两个包的已解析 feature 集逐条相符"

gate_end
