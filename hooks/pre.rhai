// pre hook：在 cargo-generate.toml 里的 placeholder 全部解析完之后运行，
// 派生两个 Liquid 变量。放在 hook 里而不是让用户再答两个问题：它们是
// 前两个名字的确定性函数，多问一次就多一次答错的机会。
//
// rhai 的字符串 API 分「就地改」与「返回新值」两族：`replace` 就地改、返回 ()，
// `to_upper` 返回新串——写错一族 variable::set 会拿到 () 而报错。
let snake = variable::get("crate_prefix");
// Rust 路径里的 crate 名：包名 `edge-core` 在代码里是 `edge_core`
snake.replace("-", "_");
variable::set("crate_prefix_snake", snake);
// 环境变量前缀：项目名的大写蛇形（`acme-api` → `ACME_API_CONFIG` / `ACME_API_VERSION`）
let crate_name = variable::get("crate_name");
variable::set("env_prefix", crate_name.to_upper());
