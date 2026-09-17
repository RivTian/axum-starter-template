# {{crate_prefix}}-http

HTTP 任务面：零路由起步的 `router()` 与可关停的 `serve(...)`。

## 边界

- 只有"请求怎么接到 axum 上、怎么优雅停服"。没有业务路由、没有 `middleware/` 目录、没有 `AppState`。
- **不依赖 runtime crate**：关停信号以 `impl Future<Output = ()>` 传入（装配层传 `ctx.cancelled()`），
  所以这一层既可以被任务面用它，也可以在测试里直接 `await` 一个 `oneshot`。
- 骨架故意没有 `/healthz` / `/ready`：就绪语义需要一个"就绪来源"，而骨架里没有消费者（没有编排器、
  没有依赖探针），预置就是假实现。加第一条业务路由时把健康检查一起加。

## 目录

- `src/serve.rs`：`router()` 与 `serve(listener, router, shutdown)`。
- `tests/serve.rs`：真实 socket 的请求/关停测试（零路由回 404、关停返回 Ok）。

## 关键决策

- **零路由**：生成结果不带 API 端点（任务书红线）；`Router::new()` 回 404 是"服务在跑"的最小事实。
- **端口不进代码**：`[http].bind` 默认 `127.0.0.1:0`（OS 分配），绑定成功后把实际 `local_addr()`
  打日志——零端口默认也能被 curl。
- **优雅关停走 axum 官方路径**：`with_graceful_shutdown` 停止接收新连接并等待在途连接收尾；
  收尾预算由 supervisor 的资源/收割阶段负责，不在这里重复造超时。
