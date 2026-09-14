# app

## 边界

装配层唯一持有配置写端、OS 信号、监督器与重载作业所有者；背景加载只返回候选值，不发布配置。

## 目录

src/main.rs、config/{mod.rs,load.rs,runtime.rs,reload.rs}、boot.rs、shutdown.rs、supervisor.rs、signals.rs、rt.rs、telemetry.rs；tests/ 与 examples/。

## 关键决策

BootConfig 与 HotConfig 类型分开；整批冷变化拒绝、热变化原子发布。重载单飞加一位合并请求；超时不释放未结束的 blocking 槽，panic 升级为进程故障，关停封提交并共用收割期限。RuntimeSet 在同步入口持有全部 runtime；Executors 只在装配时解析目标，任务退出记录保留实际 runtime。部分创建失败也走有界清理，所有 runtime 共用首次关闭期限。

## 测试形态

保留 M2 生命周期验收；增加分类/单飞/超时/迟到结果/loader panic/关停集成；debug/release × 五布局实际进程验证发布与消费、文件保持、任务运行位置、关池/关 runtime 顺序和信号竞态；同步测试验证部分构建失败、总预算与额外 worker 饱和隔离。

默认配置路径和选择策略属于 config 模块；main 仅捕获 CLI/环境/cwd 并安装进程设施。stdout 终端样式由 telemetry 决定，文件/管道保持纯文本。
