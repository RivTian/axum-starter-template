# core

## 边界

共享值、生命周期能力、热配置快照与读写能力；不读文件/环境，不建 runtime 或业务全局单例。

## 目录

src/lib.rs、lifecycle.rs、config.rs。

## 关键决策

非 Clone publisher；generation 和 HotConfig 同处一个 Arc；send_replace 保留无读者时的最新值；changed 使用 borrow_and_update，借用不跨 await。

## 测试形态

同值不增代、无读者/晚订阅、慢读者跳代、值/代号一致、关闭、代号溢出；保留生命周期与周期值测试。
