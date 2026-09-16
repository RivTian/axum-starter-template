// 生成前的身份校验。
//
// 这个钩子只做一件事：把 `crate_prefix` 定下来，并且**在生成任何文件之前**确认它是一个
// 能跑通的名字。放在这里而不是放在 `[placeholders]` 里，理由有两条：
//
//   1. 要问用户的只有"项目叫什么"，而 `--name` / 交互提示已经在问了。再放一个占位符
//      等于问两遍同一个问题，然后由模板来处理"两次答得不一样"。
//   2. 占位符只能表达"形状对不对"（一条正则），表达不了"为什么不对"。名字写错时用户
//      拿到的应该是一句能照着改的话，不是一条正则。
//
// 失败一律用 `abort(...)`：它让 cargo-generate 当场停下。**不能**只是打印一句警告——
// 那样生成会跑完，用户看到的最后一行是"generated successfully"，而目录里是个错的项目。
//
// abort 之后目标目录的状态，是一个必须写进错误消息的事实：cargo-generate 0.24.0 在跑
// 任何钩子**之前**就建好了目标目录（`src/lib.rs` 的 `destination.create()`），而渲染和
// 钩子全都发生在一个临时目录里，最后一步才整体拷回去（`copy_expanded_template`）。
// 于是 abort 留下的是一个**空目录**，不是半成品——但 `ProjectDir::create` 见到已存在的
// 路径就 `bail!("Target directory already exists")`。用户改完名字重跑，撞上的会是这句
// 与真实原因毫无关系的报错。所以下面每条 abort 都要提醒他先把那个空目录删掉。

// `crate_prefix` 的形状：`^[a-z][a-z0-9]*(-[a-z0-9]+)*$`。
//
// 这里手写字符检查而不是用正则，是因为 rhai 没有正则。检查逐条对应一个真实后果：
//   - 首字符必须是小写字母：cargo 不接受数字开头的包名。
//   - 只允许小写字母、数字、连字符：大写字母会让 `<prefix>-core` 与 cargo 规范化后的
//     名字对不上，下划线会和连字符混用（`my_svc-core` 这种名字是合法的，但它同时存在
//     两种分隔符，锁文件反向映射和人眼都容易看错）。
//   - 不允许连续连字符、不允许以连字符结尾：cargo 接受，但 `<prefix>--core` 之类的名字
//     只会让人以为自己看错了。
// 所有拒绝都走这里，为的是那句"先把空目录删掉"只写一遍、也不会漏写。
fn reject(reason) {
    abort(
        reason + "\n" +
        "  note: cargo-generate created the (empty) target directory before running\n" +
        "        this hook. remove it before retrying, otherwise the next run fails\n" +
        "        with `Target directory already exists` instead of this message."
    );
}

fn is_valid_prefix(s) {
    if s.len < 1 {
        return false;
    }
    let first = s[0];
    if first < 'a' || first > 'z' {
        return false;
    }
    let prev_dash = false;
    for i in 1..s.len {
        let c = s[i];
        if c == '-' {
            if prev_dash {
                return false;
            }
            prev_dash = true;
        } else if (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') {
            prev_dash = false;
        } else {
            return false;
        }
    }
    // 结尾是连字符的话，最后一次循环把 `prev_dash` 留成了 true。
    !prev_dash
}

// ── 取值 ────────────────────────────────────────────────────────────────────
// `--define crate_prefix=…` 显式给定时只校验、不覆盖：用户既然写出来了，就说明他不想要
// 从项目名推导的那个。没给定时按 cargo-generate 自己的大小写约定（heck）推导。
let explicit = variable::is_set("crate_prefix");
let prefix = if explicit {
    variable::get("crate_prefix")
} else {
    to_kebab_case(variable::get("project-name"))
};

// ── 校验 ────────────────────────────────────────────────────────────────────
if !is_valid_prefix(prefix) {
    let source = if explicit { "--define crate_prefix" } else { "project name" };
    reject(
        "invalid crate prefix `" + prefix + "` (derived from " + source + ").\n" +
        "  required shape: lowercase letters, digits and single hyphens,\n" +
        "                  starting with a letter, not ending with a hyphen.\n" +
        "  examples: `orders`, `orders-gateway`, `a`.\n" +
        "  this prefix becomes the package name of every workspace member,\n" +
        "  e.g. `" + prefix + "-core`, so cargo has to accept it verbatim."
    );
}

// ── 与依赖图撞名的前缀 ──────────────────────────────────────────────────────
//
// 六个成员叫 `<prefix>-{core,storage,worker,api,app,testkit}`。只要其中任何一个名字
// 在**已锁定的依赖图**里已经存在，生成出来的项目第一条命令就会红——而红的地方看不出
// 原因。机制是这样的：
//
//   `Cargo.lock` 里每条依赖引用平时写成裸名 `"axum-core"`；一旦图里出现两个同名包
//   （一个本地路径、一个 registry），cargo 就改写成带版本的消歧形式
//   `"axum-core 0.5.6"`。随模板发布的那份锁文件是在一个**不撞名**的前缀下生成的，
//   里面是裸名，于是撞名时它不再匹配——而生成结果的三条门禁全部带 `--locked`，
//   `cargo` 只会说 "cannot update the lock file because --locked was passed"。
//
// 这份名单是从随模板发布的 `Cargo.lock` 里**实测反推**出来的：取锁文件里所有以那六个
// 后缀结尾的包名，去掉后缀，剩下的就是它。它会随依赖集变化，所以模板仓库的 `structure`
// 门禁会重算一遍再和这里比对——两边对不上就红，不指望有人记得回来改。
//
// 已实测：`axum` / `sqlx` / `tracing` 三个确实会让 `--locked` 当场拒绝；
// `tokio`、`orders-gateway` 这类不撞名的 crates.io 名字则完全正常。
fn collides_with_dependency(prefix) {
    let reserved = ["axum", "futures", "sqlx", "sqlx-macros", "tracing"];
    for r in reserved {
        if r == prefix {
            return true;
        }
    }
    false
}

if collides_with_dependency(prefix) {
    reject(
        "crate prefix `" + prefix + "` collides with a package already in the\n" +
        "  dependency graph: one of this workspace's members would be named\n" +
        "  `" + prefix + "-core` (or `-api`, `-app`, ...), and a crate by that exact\n" +
        "  name is already locked.\n" +
        "  consequence: the published `Cargo.lock` spells dependency references as\n" +
        "  bare names, but cargo switches to the disambiguated `name version` form\n" +
        "  when two packages share a name. the shipped lock would stop matching, and\n" +
        "  every gate in the generated project runs with `--locked` -- so the very\n" +
        "  first command you run would fail, with a message about the lock file\n" +
        "  rather than about the name.\n" +
        "  pick another prefix. most names are fine: only `axum`, `futures`, `sqlx`,\n" +
        "  `sqlx-macros` and `tracing` are taken."
    );
}

variable::set("crate_prefix", prefix);

// 返回值会被 cargo-generate 打出来。这是用户唯一一次看到"我到底被叫成了什么"的机会——
// 后面所有的包名都是 `<prefix>-*`，而 `<prefix>` 未必等于他输入的项目名（heck 会把
// `MyService` 变成 `my-service`）。
"crate prefix: " + prefix
