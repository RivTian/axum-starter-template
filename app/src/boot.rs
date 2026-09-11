//! 引导：共享状态装配 → 顶层任务注册 → monitor。
//!
//! CLI、tracing、配置、runtime 四步在 `main` 里（它们在 runtime 之前发生），
//! 这里是进了 runtime 之后的部分。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

// 兄弟 crate 取别名而不是在表达式里全限定：`api::bind` 一眼看得出跨 crate，
// 同时又不把项目名带进表达式的宽度里（见模板仓库 README 的「rustfmt 不是名字无关的」）
use {{crate_prefix_snake}}_api as api;
use {{crate_prefix_snake}}_core::AppResult;
use {{crate_prefix_snake}}_core::config::ConfigStore;
use {{crate_prefix_snake}}_core::events::EventBus;
use {{crate_prefix_snake}}_core::metrics::Metrics;
use {{crate_prefix_snake}}_core::task::TaskSupervisor;
use {{crate_prefix_snake}}_core::tls::ensure_crypto_provider;
use {{crate_prefix_snake}}_core::util::now_ms;
use {{crate_prefix_snake}}_storage::init_storage;
use {{crate_prefix_snake}}_worker as worker;

use crate::rt::Executors;
use crate::signals::monitor;
use crate::state::RuntimeState;

pub async fn boot_strap(config: ConfigStore, executors: Executors) -> AppResult<()> {
    // rustls ring provider 尽早安装（PG TLS 与将来任何 HTTP 客户端共用）
    ensure_crypto_provider();

    // 存储：连接 → 迁移 → 健康自检，任一步失败即终止启动。
    // 放在任务注册之前：带着不可用的存储跑起来，故障会推迟到首次写入才暴露，
    // 那时已经分不清是配置错还是运行时故障。
    // 池在主 runtime 上建：它是各面共享的资源，主 runtime 最后关（跨 runtime 规则 2）
    let storage = init_storage(&config.current().storage).await?;

    // 共享状态装配
    let state = RuntimeState {
        config,
        events: EventBus::default(),
        storage,
        metrics: Arc::new(Metrics::default()),
        executors,
        shutdown: CancellationToken::new(),
    };

    // 顶层任务注册；任一注册失败即中止启动（取消已注册者 → 收割），
    // 返回原始错误——半装配的进程对外服务只会把启动问题拖成运行时问题
    let mut supervisor = TaskSupervisor::new();
    if let Err(error) = register_runtime_tasks(&state, &mut supervisor) {
        abort_boot(state, supervisor).await;
        return Err(error);
    }

    monitor(state, supervisor).await
}

/// 注册全部运行期顶层任务（全部 `child_token`，级联取消）。
///
/// **既定不变量：`http` 永远是最后一个注册的顶层任务。** HTTP 开始接流量时，
/// 其余运行面必须已经就绪，否则请求会打到半装配的运行面。其他面一律插在 http
/// **之前**；它们各自的启动闸（TOML 侧 `enabled = false` 时不注册并打 warn——
/// 「接口全都在但数据不更新」是最难猜的一种故障，关掉必须在日志里喊出来）
/// 也落在此函数内。
pub(crate) fn register_runtime_tasks(
    state: &RuntimeState,
    supervisor: &mut TaskSupervisor,
) -> AppResult<()> {
    let cfg = state.config.current();

    // ticker：示例面，先于 http。半热语义：启动即关不补拉起，重开需要重启进程。
    // 绑定到哪个 runtime由它的配置段说了算（缺省主 runtime），装配层只做解析
    if cfg.ticker.enabled {
        let executor = state.executors.resolve(cfg.ticker.runtime.as_deref())?;
        tracing::info!(
            runtime = cfg.ticker.runtime.as_deref().unwrap_or("main"),
            "registering ticker"
        );
        supervisor.register(
            "ticker",
            executor.spawn(worker::ticker(
                state.config.handle(),
                state.metrics.clone(),
                state.shutdown.child_token(),
            )),
        );
    } else {
        tracing::warn!("ticker disabled by [ticker].enabled, nothing will tick");
    }

    // http：最后注册（见函数文档的不变量）。同步 bind 在这里 fail-fast，
    // socket 的 runtime 注册在 serve 的 future 里、于目标 runtime 上完成
    let listener = api::bind(&cfg.http)?;
    let api_state = api::AppState {
        storage: state.storage.clone(),
        config: state.config.handle(),
        metrics: state.metrics.clone(),
        started_at_ms: now_ms(),
    };
    let executor = state.executors.resolve(cfg.http.runtime.as_deref())?;
    tracing::info!(
        runtime = cfg.http.runtime.as_deref().unwrap_or("main"),
        "registering http"
    );
    supervisor.register(
        "http",
        executor.spawn(api::serve(
            api_state,
            listener,
            state.shutdown.child_token(),
        )),
    );
    Ok(())
}

/// 启动中止路径的收割宽限。
///
/// 短于正常关停的宽限：走到这里的任务刚注册完、没有在途业务写入，
/// 等待只为收割 JoinHandle，不必给满正常关停的预算。
const ABORT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// 启动失败的中止序列：取消已注册任务 → 收割 → 关库。
///
/// 调用方随后返回**原始**启动错误——它是唯一有排障价值的信息。
/// 此处**不发 `ShuttingDown` 事件**：各运行面的订阅者尚未就绪，广播只会
/// 石沉大海；带事件的完整关停序列属于 monitor 的正常退出路径。
async fn abort_boot(state: RuntimeState, mut supervisor: TaskSupervisor) {
    state.shutdown.cancel();
    supervisor.wait_for_shutdown(ABORT_GRACE).await;
    // 存储最后关：与正常关停同序（先停任务再关池，避免把收尾写入变成连接错误）
    state.storage.close().await;
}

/// tracing 初始化：`RUST_LOG` 过滤（缺省 info）；compact 单行输出到 stdout。
///
/// crate 边界即过滤单元（默认 module-path target）：
/// `RUST_LOG={{crate_prefix_snake}}_worker=debug,{{crate_prefix_snake}}_api=info`。
/// 单一形态、无格式开关：compact 的 `key=value` 采集器切得动，人也扫得动。
pub(crate) fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .compact()
        // 线程名进日志：多 runtime 时一眼看出一条日志来自哪个 runtime（`compute-0`）
        .with_thread_names(true)
        .with_env_filter(filter)
        .init();
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;

    use {{crate_prefix_snake}}_core::AppError;
    use {{crate_prefix_snake}}_core::config::ConfigStore;
    use {{crate_prefix_snake}}_core::events::EventBus;
    use {{crate_prefix_snake}}_core::task::TaskSupervisor;
    use {{crate_prefix_snake}}_core::util::config_file_name;
    use {{crate_prefix_snake}}_testkit::{LogCapture, temp_storage, test_port};

    use crate::rt::Executors;
    use crate::state::RuntimeState;

    use super::{abort_boot, register_runtime_tasks};

    /// 装配一个指向临时目录（独立库文件 + 独立配置）的 RuntimeState。
    pub(crate) async fn state_at(tag: &str, port: u16) -> RuntimeState {
        state_with(tag, port, "").await
    }

    /// 同上，`extra_toml` 追加在配置末尾（如 `[ticker]` 段的开关分支）。
    pub(crate) async fn state_with(tag: &str, port: u16, extra_toml: &str) -> RuntimeState {
        let (dir, storage) = temp_storage(&format!("boot-{tag}")).await;
        let config_path = dir.join(config_file_name());
        std::fs::write(
            &config_path,
            format!("[http]\nhost = \"127.0.0.1\"\nport = {port}\n{extra_toml}"),
        )
        .expect("写测试配置");

        RuntimeState {
            config: ConfigStore::load_or_init(config_path, dir.clone()).expect("加载测试配置"),
            events: EventBus::default(),
            storage,
            metrics: Default::default(),
            executors: Executors::from_current(),
            shutdown: CancellationToken::new(),
        }
    }

    /// 收着日志跑注册，返回注册期间的日志全文
    fn register_capturing_log(state: &RuntimeState, supervisor: &mut TaskSupervisor) -> String {
        let capture = LogCapture::default();
        capture.capture(|| {
            register_runtime_tasks(state, supervisor)
                .expect("闸关时注册仍应成功（只少被关掉的那个任务）");
        });
        capture.contents()
    }

    /// 对本机端口发一个最小 HTTP GET，返回响应首行（真 TCP，不走 oneshot——
    /// 这里测的正是「bind 出来的 socket 在服务」）。
    pub(crate) async fn http_get_status_line(port: u16, path: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接测试端口");
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("发请求");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("读响应");
        String::from_utf8_lossy(&buf)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// 注册后进程驻留：两任务长活（宽限内不退出）、端口真实服务；取消后干净收割。
    #[tokio::test]
    async fn http_task_keeps_the_process_resident() {
        let port = test_port(0);
        let state = state_at("resident", port).await;
        let mut supervisor = TaskSupervisor::new();

        register_runtime_tasks(&state, &mut supervisor).expect("注册顶层任务");
        assert_eq!(supervisor.len(), 2, "ticker + http 两个顶层任务");

        // 驻留判据一：真 TCP 打探针端点，路由在服务、存储在自检
        let status = http_get_status_line(port, "/v1/service/ready").await;
        assert!(status.contains("200"), "GET 就绪探针应 200: {status}");

        // 驻留判据二：任务在观察窗口内不退出（select 的 next_exit 不该就绪）
        let idle = tokio::time::timeout(Duration::from_millis(300), supervisor.next_exit()).await;
        assert!(idle.is_err(), "顶层任务不该自行退出");

        // 取消后干净收割（graceful shutdown 响应 CancellationToken）
        state.shutdown.cancel();
        supervisor.wait_for_shutdown(Duration::from_secs(5)).await;
        state.storage.close().await;
    }

    /// `[ticker].enabled = false`：不注册 ticker，但 http 照注册（len == 1）。
    /// 日志里必须有 warn——「接口全都在但数据永不更新」是最难猜的故障，
    /// 关掉不喊出来等于给未来的排障埋雷
    #[tokio::test]
    async fn disabled_ticker_is_skipped_with_a_warning() {
        let port = test_port(3);
        let state = state_with("disabled", port, "\n[ticker]\nenabled = false\n").await;
        let mut supervisor = TaskSupervisor::new();

        let log = register_capturing_log(&state, &mut supervisor);

        assert_eq!(supervisor.len(), 1, "enabled=false 时只剩 http");
        assert!(
            log.contains("ticker disabled by [ticker].enabled"),
            "关闸必须在日志里喊出来: {log}"
        );

        state.shutdown.cancel();
        supervisor.wait_for_shutdown(Duration::from_secs(5)).await;
        state.storage.close().await;
    }

    /// bind 失败（端口被占）→ 注册报**原始** IO 错误 → abort_boot 收尾后
    /// 存储已关（门面操作失败即证明连接池真的关了）。
    #[tokio::test]
    async fn bind_failure_aborts_boot_with_the_original_error() {
        let port = test_port(1);
        // 先占住端口制造 bind 失败；绑同一地址族与地址
        let _blocker = std::net::TcpListener::bind(("127.0.0.1", port)).expect("占位监听");

        let state = state_at("bindfail", port).await;
        let storage_probe = state.storage.clone();
        let mut supervisor = TaskSupervisor::new();

        let error =
            register_runtime_tasks(&state, &mut supervisor).expect_err("端口被占应启动失败");
        assert!(
            matches!(
                &error,
                AppError::Io(io) if io.kind() == std::io::ErrorKind::AddrInUse
            ),
            "应返回原始 AddrInUse，而不是包装过的模糊错误: {error}"
        );
        // 注册顺序的行为探针：http bind 失败的瞬间 ticker 已在册 ⟺ 它注册先于 http。
        // 「http 最后注册」不变量由这个基数钉住
        assert_eq!(
            supervisor.len(),
            1,
            "bind 失败时 ticker 应已注册在先（顺序不变量）"
        );

        abort_boot(state, supervisor).await;
        assert!(
            storage_probe.health().await.is_err(),
            "abort_boot 后连接池应已关闭"
        );
    }
}
