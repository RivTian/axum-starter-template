// 生成后的收尾与边界断言。
//
// 这个钩子做两类事，第二类才是它存在的真正理由：
//
//   1. **改名**：`README.project.md` → `README.md`，`Makefile.project` → `Makefile`。
//      模板仓库自己也需要一份 README 和一份 Makefile（它们验的是"模板生成得对不对"），
//      两边不能同名，所以随交付的那两份带 `.project` 后缀，到这里才换回去。
//
//   2. **边界断言**：确认模板仓库自己的东西一件都没跟着进来。这不是多余的——
//      `cargo-generate.toml` 的 `ignore` 里写的是**字面路径**，删除实现是
//      `p.push(dir); p.push(f)` 之后按 `Path::exists()` 过滤（0.24.0 的
//      `src/ignore_me.rs`）。写错一个名字、或者顺手写成通配符，**不报错、不警告**，
//      只是静默地什么都没删。那种情况下用户的项目里会躺着本模板的设计稿。
//      唯一能在用户侧拦住它的地方就是这里。
//
// 为什么 abort 在这里是廉价的：0.24.0 的渲染和钩子全都跑在一个临时目录里，
// 只有全部成功之后才有 `copy_expanded_template` 把整棵树拷到目标目录
// （`src/lib.rs`）。所以这里中止，用户的目标目录里**一个文件都不会有**。
// 代价只有一个：目标目录本身在跑钩子之前就被建出来了，所以重试之前得先删掉它。
//
// 顺序（0.24.0 `src/lib.rs`）：
//   pre-hooks → 按 `ignore` 删文件 → 渲染 → **post-hooks** → 删掉全部 hook 文件
// 最后那一步是工具自己做的，所以 `post.rhai` **不需要自删**；它只需要把空掉的
// `hooks/` 目录一起收走。也正因为这个顺序，`hooks` 不能写进 `ignore`——
// 那会让本文件在轮到它之前就被删掉。
//
// 能力边界：rhai 的 `file` 模块只有 `exists` / `rename` / `delete` / `write` /
// `listdir`，**没有读文件内容的函数**。所以"有没有没展开的 `{{ }}`"这类审计做不到，
// 那是模板仓库门禁 `scripts/gen.sh` 的活（D51）。这里只能断言"在不在"。

// 与 `pre.rhai` 同一条理由：那句"先把空目录删掉"只写一遍。
fn reject(reason) {
    abort(
        reason + "\n" +
        "  note: cargo-generate created the (empty) target directory before running\n" +
        "        this hook. remove it before retrying, otherwise the next run fails\n" +
        "        with `Target directory already exists` instead of this message."
    );
}

// ── 边界断言 ────────────────────────────────────────────────────────────────
// 这份名单必须与 `cargo-generate.toml` 的 `ignore` 保持一致——它就是用来证明那份
// 名单真的生效了的。两边同时改，模板仓库的 `structure` 门禁会交叉比对。
fn assert_absent(path, why) {
    if file::exists(path) {
        reject(
            "`" + path + "` leaked into the generated project.\n" +
            "  " + why + "\n" +
            "  cause: the `ignore` list in `cargo-generate.toml` did not remove it.\n" +
            "         `ignore` takes literal paths, not globs, and silently skips\n" +
            "         entries that do not exist -- so a typo there fails this way.\n" +
            "  this is a template bug, not something you did wrong."
        );
    }
}

assert_absent("docs", "it is the template's own design record; your project does not need it.");
assert_absent("scripts", "those gates check the template, not the generated project.");
assert_absent(".github", "this template ships no CI config; `make check` is the only gate.");
assert_absent("rust-toolchain.toml", "pinning the toolchain is your decision, not the template's.");
assert_absent(".gitattributes", "it exists for the template's byte-for-byte comparison gate.");
assert_absent("target", "a build directory from the template checkout is never yours.");
assert_absent(".DS_Store", "Finder metadata from the template checkout is not part of the template.");

// ── 改名 ────────────────────────────────────────────────────────────────────
// 前置、后置都验。`file::rename` 失败会自己中止，所以中间那步不需要看返回值；
// 但 `file::exists` 是唯一能回答"名字写对了没有"的东西——`file::delete` 碰到不存在的
// 路径是静默 no-op，指望它报错是指望不上的。
fn promote(from, to) {
    if !file::exists(from) {
        reject(
            "`" + from + "` is missing from the template.\n" +
            "  it is listed in the `include` whitelist of `cargo-generate.toml` and is\n" +
            "  supposed to become `" + to + "`. this is a template bug."
        );
    }
    if file::exists(to) {
        reject(
            "`" + to + "` already exists, so `" + from + "` cannot be renamed onto it.\n" +
            "  the template's own `" + to + "` should have been removed by `ignore`.\n" +
            "  this is a template bug."
        );
    }
    file::rename(from, to);
    if file::exists(from) || !file::exists(to) {
        reject("renaming `" + from + "` to `" + to + "` did not take effect.");
    }
}

promote("README.project.md", "README.md");
promote("Makefile.project", "Makefile");

// 锁文件随模板发布（DP5），生成结果的三条门禁全部带 `--locked`。缺了它，用户敲的第一条
// 命令就会红，而报错说的是"lock file needs to be updated"——跟真实原因（模板发布时忘了
// 跑 `make lock`）隔着好几层。
if !file::exists("Cargo.lock") {
    reject(
        "`Cargo.lock` is missing from the generated project.\n" +
        "  every gate in the generated `Makefile` passes `--locked`, so the first\n" +
        "  command you run would fail. the template is published with a lock file;\n" +
        "  this copy was not. this is a template bug."
    );
}

// ── 收尾 ────────────────────────────────────────────────────────────────────
// 工具随后会去删 `hooks/pre.rhai` 和 `hooks/post.rhai`，但不会删它们所在的目录
// （`remove_dir_files` 只处理传进去的那几个路径）。直接把整个目录递归删掉：
// `file::delete` 对目录走的是 `remove_dir_all`，而本文件在执行前已经被
// `eval_file` 整个读进内存编译完了，删掉自己是安全的；工具那一步随后会因为
// `Path::exists()` 为假而空转。
//
// 注意：如果以后 `[hooks] post` 里加了第二个脚本，这一行必须挪到最后一个脚本里去。
file::delete("hooks");
if file::exists("hooks") {
    reject("failed to remove the template's `hooks/` directory.");
}

"renamed README.project.md -> README.md, Makefile.project -> Makefile"
