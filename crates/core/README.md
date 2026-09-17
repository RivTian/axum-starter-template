# {{crate_prefix}}-core

分层根。**不依赖任何兄弟 crate，也不依赖任何第三方**（根依赖表里没有它的条目，由
`crates/app/tests/structure.rs` 机械校验）。

## 边界

- 只有两样东西：`Error`（分类 + 消息 + 原因链）与 `Result<T, E = Error>` 别名。
- 不放业务类型，不放"以后可能会用"的共用工具；只有部分 crate 需要的东西应该沉到那些 crate
  里更靠下的那一个。
- 不依赖 tokio / serde：一旦这里引入运行时或序列化，整张依赖图的形状就被钉死了。

## 目录

- `src/lib.rs`：门面，只做 `mod` + `pub use`（窄门面由 `crates/app/tests/structure.rs` 校验）。
- `src/error.rs`：`ErrorKind` / `Error` 的实现与单元测试。

## 关键决策

- **错误为什么不是一个 trait**：分类（`ErrorKind`）是装配层用来映射退出码与日志级别的东西，
  具体类型就能表达，而且让各 crate 的返回值可以直接互换；trait 化只会把分类推迟到运行期。
- **为什么不引入 thiserror**：错误体是一个结构体（kind + message + source），
  `Display`/`Debug`/`std::error::Error` 合起来约三十行；手写少一层派生，也让 core 的依赖数为 0。
- **`Debug` 会打印 source 链**：日志用 `%err`（Display，只有分类与一句话）；`?err` 会带出底层原因，
  调用方要自己保证原因里没有敏感取值。

## 测试形态

单元测试贴在 `src/error.rs`：Display 只输出分类与消息、`source()` 链保留、`Debug` 暴露原因链、
`ErrorKind::as_str()` 稳定（日志字段与测试断言都依赖它）。
