//! ticker：按周期打点、计数的示例主循环。
//!
//! 它只做三件事——等周期、记指标、打日志——刚好够演示一个顶层任务面的全部纪律。
//! 真实的面把「记指标」换成业务动作即可；旁路纪律照旧：动作失败只记
//! `metrics.ticker.last_error` 与日志，不向上传播、不让循环退出（循环退出 =
//! first-failure = 进程退出，那是留给「面本身坏了」的信号，不是留给一次业务失败的）。

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use {{crate_prefix_snake}}_core::config::ConfigHandle;
use {{crate_prefix_snake}}_core::metrics::Metrics;
use {{crate_prefix_snake}}_core::util::now_ms;

/// ticker 主循环。`async fn` 返回的 future 是 `Send + 'static`（入参全是句柄 / Arc），
/// app 用 `Handle::spawn` 把它放到某个 runtime 上并注册进监管器。
pub async fn run(config: ConfigHandle, metrics: Arc<Metrics>, cancel: CancellationToken) {
    tracing::info!(
        interval_ms = config.current().ticker.interval_ms,
        "ticker starting"
    );
    loop {
        // 每 tick 现读：`interval_ms` 可热，热重载下一拍生效
        let interval = Duration::from_millis(config.current().ticker.interval_ms);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(interval) => {
                metrics.ticker.ticked(now_ms());
                tracing::debug!(ticks = metrics.ticker.snapshot().ticks, "tick");
            }
        }
    }
    tracing::info!("ticker stopping");
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use {{crate_prefix_snake}}_core::config::ConfigStore;
    use {{crate_prefix_snake}}_core::util::config_file_name;

    use super::*;

    /// 临时目录前缀。项目名只放在**常量声明**里：一旦让它进到表达式中间，这一行的
    /// 宽度就随项目名长短在 rustfmt 的阈值上下翻转，模板的 fmt 门禁便只对某些名字成立
    const DIR_PREFIX: &str = "{{crate_name}}_ticker";

    fn config_with_interval(tag: &str, interval_ms: u64) -> (PathBuf, ConfigStore) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("{DIR_PREFIX}_{tag}_{pid}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(config_file_name());
        std::fs::write(&path, format!("[ticker]\ninterval_ms = {interval_ms}\n")).unwrap();
        (dir.clone(), ConfigStore::load_or_init(path, dir).unwrap())
    }

    /// 在暂停的时钟下「睡」：运行时空闲时自动推进到下一个最早的定时器，被测任务
    /// 的每一拍都按顺序到期并重新武装——`time::advance` 做不到这点（它只触发
    /// 推进前已存在的定时器，中途新武装的那些不会被补上）
    async fn pass(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// 周期打点：过 350ms 恰好 3 个 tick（100 / 200 / 300）
    #[tokio::test(start_paused = true)]
    async fn ticks_at_the_configured_interval() {
        let (dir, store) = config_with_interval("ticks", 100);
        let metrics = Arc::new(Metrics::default());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(store.handle(), metrics.clone(), cancel.clone()));

        pass(350).await;
        assert_eq!(metrics.ticker.snapshot().ticks, 3);

        cancel.cancel();
        task.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 取消是第一分支：等待期收到取消立即退出，不等当前周期走完
    #[tokio::test(start_paused = true)]
    async fn cancellation_exits_without_waiting_for_the_next_tick() {
        let (dir, store) = config_with_interval("cancel", 60_000);
        let metrics = Arc::new(Metrics::default());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(store.handle(), metrics.clone(), cancel.clone()));

        tokio::task::yield_now().await;
        cancel.cancel();
        // 10ms 的超时远早于 60s 的那一拍：若取消没有抢在 sleep 之前，这里就会红
        tokio::time::timeout(Duration::from_millis(10), task)
            .await
            .expect("取消后应立即退出")
            .unwrap();
        assert_eq!(metrics.ticker.snapshot().ticks, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 热重载：新周期从下一次等待开始生效（在途的那一拍仍按旧周期到期）
    #[tokio::test(start_paused = true)]
    async fn hot_reloaded_interval_applies_from_the_next_wait() {
        let (dir, store) = config_with_interval("reload", 100);
        let metrics = Arc::new(Metrics::default());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(store.handle(), metrics.clone(), cancel.clone()));

        pass(350).await;
        assert_eq!(metrics.ticker.snapshot().ticks, 3);

        std::fs::write(store.path(), "[ticker]\ninterval_ms = 1000\n").unwrap();
        store.reload().unwrap();

        // 在途的 sleep 于 t=400 到期（第 4 拍），之后按 1000ms 重新等待：t=850 仍是 4
        pass(500).await;
        assert_eq!(metrics.ticker.snapshot().ticks, 4, "在途一拍按旧周期到期");

        // t=1850：第 5 拍在 1400 到期
        pass(1000).await;
        assert_eq!(metrics.ticker.snapshot().ticks, 5, "新周期从下一次等待生效");

        cancel.cancel();
        task.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
