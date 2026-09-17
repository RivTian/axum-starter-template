//! 二进制入口：保持薄。装配与关停编排在 lib 里（`{{crate_prefix_snake}}_app::bootstrap`），
//! 这样集成测试可以复用同一套逻辑，而不必再 spawn 一个进程。

fn main() {}
