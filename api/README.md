# api

## 边界

三个系统端点、最小 AppState、HTTP transport、响应/错误/提取器契约。

## 目录

src/lib.rs、settings.rs、state.rs、response.rs、error.rs、extract.rs、handler/；tests.rs 与 contract_tests.rs。

## 关键决策

API 只持存储读门面与生命周期读端。启动提交前不处理请求；正常 serve 返回是排空证明，外层 abort 不是。内联请求 timeout/trace，日志不记录完整 URI/header/body。

## 测试形态

Router 黑盒状态/JSON/404/405/HEAD/提取失败测试；真实连接的启动门、graceful、abort 残留；日志脱敏在独立实际服务进程验证，单元测试不安装全局 subscriber。仅测试路由的请求 panic 还验证连接结束但 HTTP 任务继续服务；不预设 supervisor 能看到所有 handler panic。
