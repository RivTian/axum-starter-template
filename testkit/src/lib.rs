//! # `{{crate_prefix}}-testkit`
//!
//! 集成测试共享件。**仅供各 crate 的 dev-dependencies 使用，不进任何发布产物的依赖图。**
//!
//! 只装 ≥ 2 处测试共用的东西：临时目录、日志捕获、按 pid 派生的端口。每样都很小，
//! 抽成 crate 只为一件事——api 与 app 的测试各写一份，两份迟早在细节上分家
//! （临时目录命名、端口偏移规则），并行测试就开始互踩。

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::MakeWriter;

use {{crate_prefix_snake}}_core::SERVICE_NAME;
use {{crate_prefix_snake}}_core::config::{SqliteConfig, StorageBackend, StorageConfig};
use {{crate_prefix_snake}}_storage::{Storage, init_storage};

/// 每用例独占的临时目录：`<tmp>/<crate_name>-<tag>-<pid>-<nanos>`。
///
/// 目录名带 pid 与纳秒：同一测试二进制内多用例并行、多个测试二进制并行，
/// 都不会撞目录。调用方用完自己删（删失败也不影响正确性，只是留垃圾）。
pub fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时钟不早于 1970")
        .as_nanos();
    // 目录名借 core 的 SERVICE_NAME 而不是就地写项目名：项目名一旦进到表达式里，
    // 这一行的宽度就随名字长短在 rustfmt 的 100 列上下翻转（见 scripts/check-fmt-portability.py）
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("{SERVICE_NAME}-{tag}-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("建测试目录");
    dir
}

/// 一用例一库：临时目录里起一个跑完迁移的 SQLite 文件库门面。
///
/// 文件库而非内存库：写走 writer 池、读走 reader 池，内存库的每条连接各自独立，
/// 会出现「写完读不到」的假绿。返回目录让调用方用完自己删。
pub async fn temp_storage(tag: &str) -> (PathBuf, Arc<dyn Storage>) {
    let dir = temp_dir(tag);
    let cfg = StorageConfig {
        backend: StorageBackend::Sqlite,
        sqlite: SqliteConfig {
            path: dir.join("test.db").to_string_lossy().into_owned(),
            ..SqliteConfig::default()
        },
        ..StorageConfig::default()
    };
    let storage = init_storage(&cfg).await.expect("起 SQLite 文件库");
    (dir, storage)
}

/// 端口按 pid 派生 + 用例内偏移：同一测试二进制内多用例共存不互抢，
/// 并行的其他测试二进制是别的进程、自然错开。**不为测试改生产 bind 逻辑**——
/// 派生端口经测试专属配置文件的 `[http].port` 喂给正常装配。
pub fn test_port(offset: u16) -> u16 {
    12000 + (std::process::id() % 20000) as u16 + offset
}

type Buffer = Arc<Mutex<Vec<u8>>>;

thread_local! {
    /// 本线程当前在往哪个缓冲收日志；`None` = 丢弃。
    static SINK: RefCell<Option<Buffer>> = const { RefCell::new(None) };
}

/// 全进程唯一的测试 subscriber，首次 `capture` 时装上，之后一直在。
///
/// # 为什么必须是全局 subscriber，而不是 `subscriber::with_default`
///
/// tracing 的 callsite「兴趣」缓存是**进程级**的：同一个 `info!` 点被另一条没装
/// subscriber 的线程命中时，缓存会被重建成「没人要」，此刻其他线程上用
/// `with_default` 装的线程局部 subscriber 就收不到这条事件。并行测试里这不是理论
/// 风险——本模板实测过：两百次循环里丢 1 条；而当兄弟用例恰好在敲同一批 callsite
/// （`Runtimes::shutdown` 的那几行）时，几乎必丢，表现为「日志少了中间几行」的假失败。
///
/// 装一个恒定存在、恒定 enabled 的全局 subscriber，兴趣缓存就稳定为「要」，
/// 事件一律送到这里，再按线程局部的 `SINK` 决定进缓冲还是进废纸篓。
fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // 与生产同形（compact、带线程名、INFO），断言照着生产真出的那行写。
        // 只关 ANSI：颜色码会把 `key=value` 拆断，断言就成了断配色
        tracing_subscriber::fmt()
            .compact()
            .with_thread_names(true)
            .with_writer(ThreadSink)
            .with_ansi(false)
            .init();
    });
}

/// 把事件路由到本线程 `SINK` 的 writer 工厂。
#[derive(Clone, Copy)]
struct ThreadSink;

enum SinkWriter {
    Buf(Buffer),
    Discard,
}

impl io::Write for SinkWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if let Self::Buf(buf) = self {
            buf.lock().expect("log buffer lock").extend_from_slice(b);
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ThreadSink {
    type Writer = SinkWriter;

    fn make_writer(&'a self) -> SinkWriter {
        SINK.with_borrow(|s| s.clone())
            .map_or(SinkWriter::Discard, SinkWriter::Buf)
    }
}

/// 收集 tracing 输出的内存缓冲。**按线程生效**：`capture` 内同步跑的代码所发的日志
/// 进缓冲，别的线程照旧丢弃，所以并行用例之间互不串味。
///
/// 跨线程的 async 任务（`spawn` 到别的 worker 上）不会被收进来——要断言这类日志，
/// 断言它的同步注册段，或让任务把结论发到 `EventBus` 而不是只打日志。
#[derive(Clone, Default)]
pub struct LogCapture(Buffer);

impl LogCapture {
    /// 在本线程把日志收进本缓冲，跑完 `f` 即恢复原状（可嵌套）。
    pub fn capture<T>(&self, f: impl FnOnce() -> T) -> T {
        install();
        let previous = SINK.with_borrow_mut(|s| s.replace(self.0.clone()));
        let _restore = Restore(previous);
        f()
    }

    /// 到目前为止捕获的全部日志文本。
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer lock")).into_owned()
    }
}

/// 出作用域（含 panic 展开）时把线程的 `SINK` 放回去。
struct Restore(Option<Buffer>);

impl Drop for Restore {
    fn drop(&mut self) {
        SINK.with_borrow_mut(|s| *s = self.0.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dirs_are_unique_and_exist() {
        let a = temp_dir("t");
        let b = temp_dir("t");
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn ports_differ_by_offset_and_stay_in_range() {
        assert_eq!(test_port(3), test_port(0) + 3);
        assert!(test_port(0) >= 12000);
    }

    #[test]
    fn capture_records_message_and_fields() {
        let capture = LogCapture::default();
        capture.capture(|| tracing::warn!(answer = 42, "captured"));
        let text = capture.contents();
        assert!(text.contains("captured"), "{text}");
        assert!(text.contains("answer=42"), "{text}");
    }

    /// `capture` 之外发的日志不进缓冲：并行用例不会互相串味。
    /// 标记词刻意不含用例名的任何词根——`with_thread_names` 会把用例名写进每一行
    #[test]
    fn events_outside_capture_are_discarded() {
        let capture = LogCapture::default();
        capture.capture(|| tracing::warn!("kept-marker"));
        tracing::warn!("dropped-marker");
        let text = capture.contents();
        assert!(text.contains("kept-marker"), "{text}");
        assert!(!text.contains("dropped-marker"), "{text}");
    }

    /// 另一条线程持续敲同一个 callsite 时也一条不丢——这正是
    /// `subscriber::with_default` 做不到的（见 `install` 的注释）
    #[test]
    fn capture_survives_concurrent_hits_on_the_same_callsite() {
        use std::sync::atomic::{AtomicBool, Ordering};

        fn emit(tag: &str) {
            tracing::info!(tag = %tag, "shared callsite");
        }

        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let noise = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                emit("noise");
            }
        });

        let mut lost = 0;
        for _ in 0..200 {
            let capture = LogCapture::default();
            capture.capture(|| emit("wanted"));
            if !capture.contents().contains("wanted") {
                lost += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        noise.join().expect("噪声线程");
        assert_eq!(lost, 0, "并行敲同一 callsite 丢了 {lost}/200 条");
    }
}
