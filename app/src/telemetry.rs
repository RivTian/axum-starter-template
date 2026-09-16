//! 遥测：全仓库**唯一**安装 subscriber 的地方。
//!
//! # 为什么库层一行都不能碰
//!
//! 装 subscriber 是改**全局**。一个库如果在 `init()` 里顺手装了，那么任何 `use` 它的
//! 程序都会被它擅自接管日志去向——而这类副作用在出问题时几乎不可能从调用点看出来。
//! 所以 `tracing-subscriber` 在根清单里就只有 `app` 与 `testkit` 能依赖。
//!
//! # 装一次，而不是装了再换
//!
//! 本模板**不**装 `tracing-subscriber` 的 `reload::Layer`。它能让过滤器在运行时热换，
//! 代价是每一条日志都要多过一层 `RwLock`，而这个模板里没有任何一个场景需要在不重启的
//! 前提下改日志级别——`[telemetry]` 整段都是冷字段，改了本来就要重启。
//!
//! 这条选择带来一个直接后果，写在这里以免下次有人以为是 bug：**`init` 必须在读完配置
//! 之后才能调用**，因为过滤器和格式的取值就在配置文件里。于是配置读取本身是唯一一个
//! 发生在遥测就绪之前的可失败步骤。它的失败不会丢——`run` 把读取的 `Result` 拿在手上，
//! 先用「默认遥测设置」把 subscriber 装起来，再把失败作为结构化事件报出去。
//! 换句话说，「启动失败以结构化事件出现，而不是 `eprintln!`」这条性质仍然成立，只是
//! 顺序上绕了一下，理由就是上面这一段。
//!
//! # 过滤器写错了怎么办
//!
//! 退回默认过滤器，然后**留一条 `warn`**。不能静默——"我明明把级别调成了 debug
//! 却什么都没多出来"是最难自查的一类问题，因为现场看起来完全正常。

use std::io;

use service_core::config::{LogFormat, TelemetryConfig};
use tracing::Subscriber;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

/// 过滤器不合法时退回的取值。
///
/// 和 `TelemetryConfig::default().filter` 保持一致；写成常量是为了让退回这件事在代码里
/// 有个名字，而不是一个裸字符串。
const FALLBACK_FILTER: &str = "info";

/// 过滤器最终用的是哪一个。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterOutcome {
    /// 配置里写的那个，解析通过。
    Applied,
    /// 配置里写的那个解析失败，已退回 [`FALLBACK_FILTER`]。
    FellBack {
        /// 消毒过的原文——它来自配置文件，是用户输入。
        requested: service_core::config::FieldPath,
        /// 解析器给的原因。
        reason: String,
    },
}

/// 安装全局 subscriber，并把「过滤器有没有退回」交回给调用方去记事件。
///
/// 这里**不**自己记那条 `warn`：事件得等 subscriber 装好之后才发得出去，而这个函数
/// 返回的那一刻正好就是装好的那一刻。让调用方记，顺序才是对的。
///
/// # Errors
///
/// 全局 subscriber 已经被装过时返回错误。进程里这只会发生在"`run` 被调用了两次"，
/// 而那是调用方的错误，不是可以吞掉的偶发情况。
pub(crate) fn init(
    cfg: &TelemetryConfig,
    stderr_is_terminal: bool,
) -> Result<FilterOutcome, TelemetryError> {
    let (subscriber, outcome) = build(cfg, stderr_is_terminal, io::stderr);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|err| TelemetryError::AlreadyInstalled(err.to_string()))?;
    Ok(outcome)
}

/// 安装失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum TelemetryError {
    /// 全局 subscriber 已被安装。
    #[error("a global tracing subscriber is already installed: {0}")]
    AlreadyInstalled(String),
}

/// 按配置搭出一个 subscriber。
///
/// 写成"搭"和"装"两步，是为了让格式能被**逐字段**验证：用例给一个内存 writer，
/// 拿到真正的输出字节去比对。如果只有 `init`，那条断言就只能靠盯着终端看。
///
/// 日志一律去 **stderr**。stdout 留给程序自己的输出（这里只有 `--help` / `--version`），
/// 于是 `prog --version | cat` 不会被日志污染。
fn build<W>(
    cfg: &TelemetryConfig,
    ansi: bool,
    writer: W,
) -> (Box<dyn Subscriber + Send + Sync>, FilterOutcome)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let (filter, outcome) = match EnvFilter::try_new(&cfg.filter) {
        Ok(filter) => (filter, FilterOutcome::Applied),
        Err(err) => (
            EnvFilter::new(FALLBACK_FILTER),
            FilterOutcome::FellBack {
                requested: service_core::config::FieldPath::new(&cfg.filter),
                reason: err.to_string(),
            },
        ),
    };

    let base = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(ansi)
        .with_target(true)
        .with_level(true);

    let subscriber: Box<dyn Subscriber + Send + Sync> = match cfg.format {
        LogFormat::Text => Box::new(base.finish()),
        // `flatten_event` 把 `fields` 里的键提到顶层。不这么做的话，每一条日志都会
        // 多套一层 `"fields": { ... }`，而下游的日志系统按顶层键建索引。
        LogFormat::Json => Box::new(base.json().flatten_event(true).finish()),
    };

    (subscriber, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::Level;

    /// 一个把字节收进内存的 writer。
    #[derive(Clone, Debug, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'w> MakeWriter<'w> for SharedBuf {
        type Writer = Self;
        fn make_writer(&'w self) -> Self::Writer {
            self.clone()
        }
    }

    fn emit_with(cfg: &TelemetryConfig, ansi: bool) -> (String, FilterOutcome) {
        let buf = SharedBuf::default();
        let (subscriber, outcome) = build(cfg, ansi, buf.clone());
        tracing::subscriber::with_default(subscriber, || {
            tracing::event!(
                name: "probe_event",
                Level::INFO,
                answer = 42,
                who = "cli",
                "hello"
            );
        });
        (buf.text(), outcome)
    }

    #[test]
    fn text_format_carries_level_target_fields_and_message() {
        // 逐字段验。不是"输出非空"，而是每一样该在的东西都点名。
        let cfg = TelemetryConfig {
            filter: "info".to_owned(),
            format: LogFormat::Text,
        };
        let (out, outcome) = emit_with(&cfg, false);
        assert_eq!(outcome, FilterOutcome::Applied);
        assert!(out.contains("INFO"), "级别：{out}");
        assert!(out.contains("service_app::telemetry"), "target：{out}");
        assert!(out.contains("hello"), "消息：{out}");
        assert!(out.contains("answer=42"), "数字字段：{out}");
        assert!(out.contains("who=\"cli\""), "字符串字段：{out}");
    }

    #[test]
    fn json_format_puts_fields_at_the_top_level() {
        let cfg = TelemetryConfig {
            filter: "info".to_owned(),
            format: LogFormat::Json,
        };
        let (out, _) = emit_with(&cfg, false);
        assert!(out.contains("\"level\":\"INFO\""), "{out}");
        assert!(
            out.contains("\"target\":\"service_app::telemetry::tests\""),
            "{out}"
        );
        assert!(out.contains("\"message\":\"hello\""), "{out}");
        // flatten_event 的效果：字段在顶层，不在 "fields" 下面。
        assert!(out.contains("\"answer\":42"), "{out}");
        assert!(!out.contains("\"fields\""), "字段不该再套一层：{out}");
    }

    #[test]
    fn ansi_is_decided_by_the_injected_terminal_flag_only() {
        // 两支都对着内存 buffer 验过，而不是"在我的终端上看着是彩色的"。
        const ESC: char = '\u{1b}';
        let cfg = TelemetryConfig {
            filter: "info".to_owned(),
            format: LogFormat::Text,
        };
        let (plain, _) = emit_with(&cfg, false);
        let (colored, _) = emit_with(&cfg, true);
        assert!(!plain.contains(ESC), "非终端不该着色：{plain:?}");
        assert!(colored.contains(ESC), "终端必须着色：{colored:?}");
    }

    #[test]
    fn an_invalid_filter_falls_back_and_says_so() {
        // 兜底可以，静默不行。
        //
        // 用例输入挑的是**级别名写错**，不是一串乱码。`EnvFilter` 比看上去宽容得多：
        // 一个裸词会被当成 target 名收下，所以 `"this is not a filter!!"` 其实解析得过。
        // 真正过不去的是 `target=级别` 里那个级别不认识——而那恰好也是现场最常犯的错
        // （`warning` 写成了 `warn` 的反面、`verbose` 根本不存在）。
        let cfg = TelemetryConfig {
            filter: "service_app=verbose".to_owned(),
            format: LogFormat::Text,
        };
        let (out, outcome) = emit_with(&cfg, false);
        match outcome {
            FilterOutcome::FellBack { requested, reason } => {
                assert!(!reason.is_empty(), "退回必须带原因");
                assert!(
                    requested.as_str().contains("verbose"),
                    "原文要留下来：{requested}"
                );
            }
            FilterOutcome::Applied => panic!("非法过滤器不该被当成合法的"),
        }
        // 退回到 info 之后，INFO 事件仍然出得来——否则"退回"等于"静音"。
        assert!(out.contains("hello"), "{out}");
    }
}
