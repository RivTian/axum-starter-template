// pre hook：派生变量 + 校验两个输入的形状。
// 这里 abort 是廉价的：渲染与钩子都在临时目录里跑，失败不会写坏用户的目录；
// 但目标目录在钩子之前就被创建了——abort 之后重试要先删掉它，否则下一步报的错与真实原因无关。

let prefix = variable::get("crate_prefix");
let crate_name = variable::get("crate_name");

// ── crate_prefix：包名前缀（svc-core / svc-storage / ...）────────────────────────
// cargo-generate.toml 的 regex 已经拦过一遍互动输入；这里是权威校验，消息给出正确示例。
if prefix.len() == 0 {
    abort("crate_prefix 不能为空。它是包名前缀，例如 svc → svc-core / svc-storage / svc-app");
}
if prefix.len() > 32 {
    abort("crate_prefix 最长 32 个字符（当前 "
        + prefix.len()
        + " 个）：它会出现在包名、环境变量名前缀与代码路径里，太长会更难用");
}
let allowed = "abcdefghijklmnopqrstuvwxyz0123456789_-";
let chars = prefix.to_chars();
if !"abcdefghijklmnopqrstuvwxyz".contains(chars[0]) {
    abort("crate_prefix 必须以小写字母开头（cargo 拒绝数字开头的包名）：收到 `"
        + prefix
        + "`，例如 svc");
}
for ch in chars {
    if !allowed.contains(ch) {
        abort("crate_prefix 只允许小写字母、数字、`-`、`_`：收到 `" + prefix + "`，例如 svc-core");
    }
}
if prefix.ends_with("-") || prefix.ends_with("_") {
    abort("crate_prefix 不能以分隔符结尾：收到 `" + prefix + "`，例如 svc-core");
}
if prefix.contains("--") || prefix.contains("__") || prefix.contains("-_") || prefix.contains("_-") {
    abort("crate_prefix 里出现了连续或混用的分隔符：收到 `"
        + prefix
        + "`，请用单个 - 或 _ 分隔单词，例如 svc-core");
}

// ── crate_name：来自 --name 的 snake_case 形态（二进制名与环境变量前缀的来源）──
if crate_name.len() == 0 {
    abort("crate_name 为空：请用 --name <project-name> 指定项目名");
}
if crate_name.contains("-") {
    abort("crate_name 应该是 snake_case（由 cargo-generate 从 --name 派生），收到 `"
        + crate_name
        + "`");
}

// ── 派生：代码里的 Rust 路径形态与环境变量前缀 ──────────────────────────────────
variable::set("crate_prefix_snake", prefix.to_snake_case());
variable::set("env_prefix", crate_name.to_shouty_snake_case());
