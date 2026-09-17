// post hook：渲染已完成。这里做文件级边界断言，然后收走 hooks/ 空目录。
// 内容级审计（残留占位符、里程碑词、模板门禁目标名）由模板侧 tools/audit 在生成结果上跑：
// rhai 的 file 模块只能查存在/改名/删除，读不了文件内容。

// 生成侧必须存在的东西：缺任何一个都说明渲染或 .liquid 遮蔽出了问题。
let required = [
    "Cargo.toml",
    "Makefile",
    "README.md",
    ".gitignore",
    "crates/app",
    "crates/config",
    "crates/core",
    "crates/http",
    "crates/runtime",
    "crates/storage",
    "crates/app/src/main.rs",
    "crates/storage/migrations",
];
for path in required {
    if !file::exists(path) {
        abort("生成结果缺少 `" + path + "`：渲染或 .liquid 遮蔽出了问题，检查 cargo-generate.toml 与 hooks/");
    }
}

// 模板内部材料不能出现在生成结果里：docs/scripts/tools 走 ignore，*.liquid 应被去后缀。
let forbidden = [
    "docs",
    "scripts",
    "tools",
    "cargo-generate.toml",
    "Makefile.liquid",
    "README.md.liquid",
    ".gitignore.liquid",
];
for path in forbidden {
    if file::exists(path) {
        abort("生成结果里出现了模板内部材料 `"
            + path
            + "`：检查 cargo-generate.toml 的 ignore 与 .liquid 遮蔽是否失效");
    }
}

// hooks/ 里的钩子文件由 cargo-generate 在本脚本之后删除，目录要我们自己收走。
file::delete("hooks");
if file::exists("hooks") {
    abort("hooks/ 空目录没有被收走（file::delete 失效？）");
}
