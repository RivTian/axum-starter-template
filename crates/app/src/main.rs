//! 二进制入口：保持薄。装配与关停编排在 lib 里（`{{crate_prefix_snake}}_app::run`），
//! 这样集成测试可以复用同一套逻辑，而不必再 spawn 一个进程。

use clap::Parser;

fn main() -> std::process::ExitCode {
    let args = {{crate_prefix_snake}}_app::Args::parse();
    {{crate_prefix_snake}}_app::run(args).into()
}
