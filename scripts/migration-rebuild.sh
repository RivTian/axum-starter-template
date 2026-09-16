#!/usr/bin/env bash
# 证明「改了迁移目录就一定会重新编译」——增 / 改 / 删各证一次，外加一条负向对照。
#
#   scripts/migration-rebuild.sh [--quiet]
#
# ## 它拦的是什么
#
# `sqlx::migrate!("./migrations")` 在**编译期**把整个目录读进二进制，运行期不再回磁盘。
# 而 cargo 默认只盯着 `.rs` 文件。两件事凑在一起就是一种很难查的失败：迁移文件明明改了，
# 跑起来的却还是上一次编进去的那一套，**没有任何报错**。
#
# `storage/build.rs` 里那行 `cargo::rerun-if-changed=migrations` 就是为这件事存在的。它是
# 一行看上去可以随手删掉的样板——所以这条门禁的正事不是"验证 cargo 的行为"，是**把那行
# 的必要性钉成一条会红的断言**：谁删了它，这里当场红，而红的文字会说清为什么不能删。
#
# ## 为什么先跑一趟"不该重编"
#
# 「重编了」这个观察要有意义，前提是它**能**变成「没重编」。一个永远为真的断言等于没有
# 断言：假如观察方法本身坏了（比如 `Compiling` 这行字换了措辞、或者 `-p` 选错了包），
# 三条正向断言会一起变成永远绿。所以第一步先证明这个观察能够指出"没重编"——它是后面
# 三条的前提，不是凑数的一项。
#
# ## 负向对照为什么用"新增"而不是"修改"
#
# 因为这两种改动在**没有 build.rs** 时的行为不一样，而只有一种是静默的：
#
#   - 修改一条已有迁移：sqlx 的宏为每个已存在的文件留下了编译期依赖，改它多半仍会触发
#     重编——这条不适合当负向对照，它不够稳。
#   - 新增一条迁移：那个文件在上一次编译时根本不存在，没有任何东西"依赖"它。于是新迁移
#     被完全忽略，而数据库不会多出一张表，日志里也不会多出一行。
#
# 后者正是 `build.rs` 的文档注释声称的那件事。这里把那句声称跑一遍——一条写在注释里的
# 实测结论，如果没有任何东西重跑它，过两年就只是一句传说。

# shellcheck source=scripts/lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HERE="$(dirname "${BASH_SOURCE[0]}")"

# 专用的一次性名字。这条门禁会往生成树里塞文件、还会删掉 `build.rs`，不能和别的门禁
# 共用一棵树——共用的话，谁先跑完谁决定后面那个看到的是什么。
MIG_NAME="migprobe"

while [ $# -gt 0 ]; do
    case "$1" in
        --quiet) GATE_QUIET=1; shift ;;
        -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "不认识的参数：$1" ;;
    esac
done

gate_covers "baseline"         "什么都不改时第二趟不重编——先证明这个观察能变绿"
gate_covers "add"              "新增一条迁移强制重编"
gate_covers "modify"           "改一条迁移的内容强制重编"
gate_covers "delete"           "删掉一条迁移强制重编"
gate_covers "build-rs-load-bearing" "负向：拿掉 build.rs 之后，新增迁移被静默忽略"
gate_begin  "migration-rebuild"

gate_workspace_init
gate_lock

DEST="$GATE_ROOT/gen/$MIG_NAME"
WORK="$GATE_ROOT/migration-rebuild"
mkdir -p -- "$WORK"

bash "$HERE/gen.sh" --name "$MIG_NAME" --quiet

# 前缀从生成树里读，不从 `$MIG_NAME` 推——理由同 `matrix.sh`。
PREFIX="$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\(.*\)-core"[[:space:]]*$/\1/p' \
    "$DEST/core/Cargo.toml" | head -1)"
[ -n "$PREFIX" ] || die "migration-rebuild：从 $DEST/core/Cargo.toml 里读不出包名前缀"

MIGRATIONS="$DEST/storage/migrations"
PROBE="$MIGRATIONS/0002_migration_rebuild_probe.sql"
[ -d "$MIGRATIONS" ] || die "migration-rebuild：$MIGRATIONS 不存在。
   迁移目录换了位置的话，storage/build.rs 里的 rerun-if-changed 路径也得跟着换，
   而这条门禁正是那一对的唯一看守。"

# 编译缓存放在门禁工作区里。第三方依赖（含 bundled SQLite 的那段 C 编译）因此和别的门禁
# 共用，只有两个本地 crate 需要真的编一次。
export CARGO_TARGET_DIR="$GATE_ROOT/cache"

# 跑一趟构建，回答一个是非题：`storage` 这一层重编了没有。
#
# 只 `-p <前缀>-storage`，不整个工作区：要观察的就是这一层，编 `app` 只是白等。
storage_rebuilt() {
    ( cd "$DEST" && cargo build -p "$PREFIX-storage" --locked ) > "$WORK/build.log" 2>&1 \
        || { sed 's/^/   /' "$WORK/build.log"; die "migration-rebuild：构建失败（见上）。
   这条门禁只回答「重编了没有」，构建本身就该是绿的。"; }
    grep -q "Compiling $PREFIX-storage v" "$WORK/build.log"
}

expect_rebuild() {  # $1 = 这一步叫什么，$2 = 为什么非重编不可
    storage_rebuilt || die "$1：改了迁移目录，storage 这一层却**没有**重新编译。
   后果：二进制里带的还是上一次编进去的那一套迁移，而且没有任何报错——
   $2
   看一眼 $DEST/storage/build.rs 还在不在，里面那行
   \`cargo::rerun-if-changed=migrations\` 还在不在。"
}

expect_fresh() {  # $1 = 这一步叫什么，$2 = 多余的重编意味着什么
    if storage_rebuilt; then
        die "$1：这一步**不该**重编，但它重编了。
   $2"
    fi
}

# ── baseline ─────────────────────────────────────────────────────────────────
#
# 第一趟把 `storage` 和它的依赖编出来（bundled SQLite 那段 C 也在里面，是这条门禁里最慢
# 的一段）。第二趟什么都不改，必须是全新鲜的。
storage_rebuilt || true
expect_fresh "baseline" "说明有东西让 \`storage\` 每次都脏——可能是 build.rs 打印了
   一条会变的 \`rerun-if-changed\`（比如指向 target 目录里的东西），也可能是某个
   代码生成步骤每次写出不同的字节。这两种都会让后面三条断言永远为真，
   于是这条门禁就再也拦不住任何东西了。"
ok "baseline：无改动时不重编（观察方法可以变绿）"

# ── add ──────────────────────────────────────────────────────────────────────
cat > "$PROBE" <<'SQL'
-- 门禁探针。由 scripts/migration-rebuild.sh 写入，同一次运行里会被删掉。
CREATE TABLE migration_rebuild_probe (id INTEGER PRIMARY KEY);
SQL
expect_rebuild "add" "新增的那条迁移根本不会被应用，缺的表要等到第一次查询才炸。"
ok "add：新增迁移触发重编"

# ── modify ───────────────────────────────────────────────────────────────────
#
# 改内容，不改文件名。sqlx 运行期是按**校验和**认迁移的，改了内容却没重编的话，
# 二进制里带的校验和和文件里的对不上，而对不上这件事只有部署到已有库上才会暴露。
printf '%s\n' "-- 探针第二版：内容变了，文件名没变。" >> "$PROBE"
expect_rebuild "modify" "改动不会进二进制；而 sqlx 是按校验和认迁移的，
   这种偏差要等到部署上一个已经跑过旧版的库才会炸。"
ok "modify：改动迁移内容触发重编"

# ── delete ───────────────────────────────────────────────────────────────────
/bin/rm -f -- "$PROBE"
expect_rebuild "delete" "删掉的那条迁移还留在二进制里，下一个空库仍会被它建出表来。"
ok "delete：删除迁移触发重编"

# ── build-rs-load-bearing ────────────────────────────────────────────────────
#
# 到这里为止证明的是"有 build.rs 时三种改动都会重编"。这一步证明反面：**没有它就不会**。
# 两者合起来才说明那行 `rerun-if-changed` 是承重的，而不是一句好看的样板。
/bin/rm -f -- "$DEST/storage/build.rs"

# 拿掉 build.rs 本身改变了包的指纹，这一趟必然重编——不对它断言，它只是为了把缓存重新
# 烘热，好让下一趟的"没重编"是干净的结论。
storage_rebuilt || true
expect_fresh "build-rs-load-bearing（预热）" "拿掉 build.rs 之后连着两趟都在重编，
   说明还有别的东西让这一层每次都脏。先查出那个东西，这条对照才有意义。"

cat > "$PROBE" <<'SQL'
-- 负向对照：这条迁移是在没有 build.rs 的情况下新增的。
CREATE TABLE migration_rebuild_probe (id INTEGER PRIMARY KEY);
SQL
if storage_rebuilt; then
    die "build-rs-load-bearing：没有 build.rs，新增迁移**竟然**也触发了重编。
   这不是坏消息，是一条过期的结论：说明 cargo 或 sqlx 已经自己盯住了迁移目录，
   而 \`storage/build.rs\` 与 \`storage/migrations/README.md\` 里写的理由不再成立。
   要么找出新的机制、把理由改写成它，要么确认 build.rs 可以删——
   但**不要**把这条断言改松，那等于把一条已经变化的事实盖回去。"
fi
ok "build-rs-load-bearing：没有它，新增的迁移被静默忽略"

safe_rm_rf "$DEST"
gate_end
