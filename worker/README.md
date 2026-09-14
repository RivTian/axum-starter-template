# worker

## 边界

唯一 ticker 任务面，持配置读端与 child token；不自行 spawn，不取得配置写权。

## 目录

src/lib.rs。

## 关键决策

等待启动提交后按最新快照启动；变更时重建 interval，不能只 reset_at 而保留旧 period。取消优先于配置变化和 tick；记录已消费代号，writer 意外消失返回错误。

## 测试形态

虚拟时间验证首拍、Skip、新周期的连续两拍、重复代号不重置；writer 关闭与取消优先级；真实进程验证消费日志。
