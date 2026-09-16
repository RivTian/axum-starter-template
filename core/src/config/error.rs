//! 配置错误。
//!
//! 一条贯穿本模块的纪律：**错误信息里不出现配置取值。**
//!
//! 配置错误是新项目最常见的第一类失败，因此它的消息几乎必然会被贴进工单、粘进聊天、
//! 收进日志聚合。一旦取值能进这条消息，数据库口令就迟早会跟着进去。所以这里回显的
//! 是**字段路径**，不是值。
//!
//! 路径本身也来自文件内容，因此同样要过一遍消毒：控制字符会破坏日志的行结构
//! （一个 `\n` 就能伪造出一条日志），超长路径会把一行日志顶到几 KB。[`FieldPath`]
//! 是唯一的构造途径，消毒因此是结构性的，不是"记得调一下"。

use std::fmt;

/// 路径与消息的长度上限（字符数，不是字节数——截断不能切断多字节字符）。
const MAX_LEN: usize = 128;
/// 截断标记。出现它就说明原文更长，不会有人误以为这是完整路径。
const TRUNCATED: &str = "…";

/// 一个消毒过的配置字段路径，例如 `http.bind_addr`。
///
/// 只能经 [`FieldPath::new`] 构造：没有 `From<String>`，也没有公开字段。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldPath(Box<str>);

impl FieldPath {
    /// 消毒并截断。
    #[must_use]
    pub fn new(raw: &str) -> Self {
        Self(sanitize(raw))
    }

    /// 消毒后的路径。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FieldPath({:?})", &self.0)
    }
}

/// 一段消毒过的、**保证不含配置取值**的说明。
///
/// 这个保证不是这个类型给的——它给的只是消毒。**保证由构造点给**：
///
/// - `core` 自己产生的说明全是 `&'static str` 字面量，值进不去（编译期）。
/// - 反序列化说明由 `app` 用 `serde_path_to_error` 的结果构造。serde 的消息在
///   类型不符时可能带上原值，因此凡是可能承载敏感内容的字段都必须是
///   [`Secret`](crate::config::Secret)——它手写的 `Deserialize` 永不把取值放进错误
///   （见 `secret.rs` 的 `secret_deserialize_error_never_contains_the_value`）。
///
/// 把这条边界写在这里，是因为它是整条链上唯一一处**靠约定**而非靠类型的地方。
#[derive(Clone, PartialEq, Eq)]
pub struct SafeMessage(Box<str>);

impl SafeMessage {
    /// 消毒并截断。
    #[must_use]
    pub fn new(raw: &str) -> Self {
        Self(sanitize(raw))
    }

    /// 消毒后的说明。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SafeMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SafeMessage({:?})", &self.0)
    }
}

/// 剥离控制字符（含换行与制表）并按字符数截断。
fn sanitize(raw: &str) -> Box<str> {
    let mut out = String::with_capacity(raw.len().min(MAX_LEN));
    for ch in raw.chars().filter(|c| !c.is_control()) {
        if out.chars().count() >= MAX_LEN {
            out.push_str(TRUNCATED);
            break;
        }
        out.push(ch);
    }
    out.into_boxed_str()
}

/// 配置在加载或重载过程中失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// 环境变量展开阶段失败。
    #[error(transparent)]
    Expand(#[from] crate::config::ExpandError),

    /// 反序列化失败。由调用方（`app`）从 `serde_path_to_error` 的结果构造。
    ///
    /// `core` 不依赖任何具体格式的解析库——这是"第三方 crate 的所在层受限"的直接后果：配置从哪种格式来是
    /// 进程事实，而 `core` 只定义类型与管线。
    #[error("config field `{path}` could not be parsed: {message}")]
    Deserialize {
        /// 出问题的字段路径。
        path: FieldPath,
        /// 消毒过的说明。
        message: SafeMessage,
    },

    /// 校验失败：**继续跑必然失败**。
    ///
    /// 只有这一类会拦下启动。越界但能跑的一律走 `clamp` + `warn`，不在这里。
    #[error("config field `{path}` is invalid: {reason}")]
    Invalid {
        /// 出问题的字段路径。
        path: FieldPath,
        /// 为什么它必然失败。`&'static str`：用户输入在编译期就进不来。
        reason: &'static str,
    },

    /// 敏感取值取不到。
    #[error("secret at `{path}` could not be resolved from {tried}")]
    Secret {
        /// 出问题的字段路径。
        path: FieldPath,
        /// 试的是哪一路来源。
        ///
        /// 字段名刻意不叫 `source`：`thiserror` 把这个名字当成"错误的上游错误"，
        /// 会去要求它实现 `std::error::Error`。这是个会让人愣一下的编译错误，写在这里省一次。
        tried: &'static str,
    },
}

impl ConfigError {
    /// 报告与日志里使用的稳定短名。
    #[must_use]
    pub const fn kind_str(&self) -> &'static str {
        match self {
            Self::Expand(_) => "expand",
            Self::Deserialize { .. } => "deserialize",
            Self::Invalid { .. } => "invalid",
            Self::Secret { .. } => "secret",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_stripped() {
        // 一个换行就能在日志里伪造出一条记录。路径来自文件内容，必须消毒。
        let cases: &[(&str, &str)] = &[
            ("http.bind_addr", "http.bind_addr"),
            ("http\n.bind_addr", "http.bind_addr"),
            ("a\tb", "ab"),
            ("a\r\nERROR fake log line", "aERROR fake log line"),
            ("a\u{0}b", "ab"),
        ];
        for (i, (raw, expected)) in cases.iter().enumerate() {
            assert_eq!(
                FieldPath::new(raw).as_str(),
                *expected,
                "TC{i} 的消毒结果不符"
            );
        }
    }

    #[test]
    fn overlong_paths_are_truncated_with_a_visible_marker() {
        let long = "a".repeat(MAX_LEN * 3);
        let path = FieldPath::new(&long);
        assert!(path.as_str().ends_with(TRUNCATED), "截断必须留下可见标记");
        assert_eq!(path.as_str().chars().count(), MAX_LEN + 1);
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // 按字节截断会切出非法 UTF-8；这里按字符截断。
        let long = "字".repeat(MAX_LEN * 2);
        let path = FieldPath::new(&long);
        assert!(
            path.as_str()
                .chars()
                .all(|c| c == '字' || c.to_string() == TRUNCATED)
        );
    }

    #[test]
    fn config_error_never_echoes_values() {
        // 回归用例：错误的 Display 里只有路径和 &'static str 的理由。
        let err = ConfigError::Invalid {
            path: FieldPath::new("storage.path"),
            reason: "path has no file name",
        };
        let rendered = err.to_string();
        assert!(rendered.contains("storage.path"));
        assert!(rendered.contains("path has no file name"));
        assert_eq!(err.kind_str(), "invalid");
    }
}
