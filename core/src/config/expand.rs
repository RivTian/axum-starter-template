//! 环境变量展开：**只认 `${NAME}` 和 `${NAME:default}`，别的一律报错。**
//!
//! 展开发生在解析之前，作用在配置文件的**原始文本**上。这样做的代价是它看不见 TOML 的
//! 结构（不知道自己正站在字符串里还是注释里），换来的是它与格式无关——`core` 因此不必
//! 认识 TOML。这笔交换是"第三方 crate 的所在层受限"的直接后果。
//!
//! # 为什么不支持更多语法
//!
//! shell 里有 `${A:-b}`、`${A:?err}`、`${A/x/y}` 十来种形式。支持其中一部分、把其余当
//! 字面量放过去，是这类展开器最常见的坑：运维照着 shell 的肌肉记忆写了 `${A:-b}`，
//! 得到的是一串**原样保留**的文本，然后花半天去查"为什么端口是 `${A:-b}`"。
//!
//! 所以这里的规则是**认识的就展开，不认识的就报错**，没有"原样放过"这一档。
//! 多支持一种形式是后续的增量；把错误当默认值静默用下去是一整天的排查。
//!
//! # `$` 本身
//!
//! 口令里出现 `$` 很常见，因此需要一个转义：**`$$` 展开成一个 `$`**。裸 `$` 后面跟任何
//! 东西都是错误——它比"猜运维想要什么"安全，而且错误消息能直接告诉他写 `$$`。

use std::collections::BTreeMap;

use super::error::FieldPath;

/// 展开时的变量来源。
///
/// 是个 trait 而不是直接收 `BTreeMap`，因为 `core` **不读进程环境**：真实的
/// `std::env::vars()` 只在 `app` 的 `ProcessEnv::capture()` 里出现一次。测试给一个
/// 内存实现，进程给一个环境实现，展开逻辑本身两边同一份。
pub trait VarSource {
    /// 取变量。不存在返回 `None`（与"存在但为空串"必须区分开）。
    fn get(&self, name: &str) -> Option<&str>;
}

impl VarSource for BTreeMap<String, String> {
    fn get(&self, name: &str) -> Option<&str> {
        BTreeMap::get(self, name).map(String::as_str)
    }
}

impl<T: VarSource + ?Sized> VarSource for &T {
    fn get(&self, name: &str) -> Option<&str> {
        (**self).get(name)
    }
}

/// 展开阶段的失败。
///
/// 每个变体都带 `at`（字节偏移），因为展开发生在解析之前，这时还没有"字段路径"这个概念——
/// 唯一能给出的定位就是偏移。调用方把它连同文件名一起报出去，运维能直接跳到出错的位置。
#[derive(Debug, thiserror::Error)]
pub enum ExpandError {
    /// `${` 没有对应的 `}`。
    #[error("unterminated `${{` at byte offset {at}")]
    Unterminated {
        /// `$` 的字节偏移。
        at: usize,
    },

    /// 变量名非法（空，或含标识符以外的字符）。
    #[error(
        "invalid variable name `{name}` at byte offset {at}: expected `[A-Za-z_][A-Za-z0-9_]*`"
    )]
    InvalidName {
        /// 消毒过的原文（可能来自配置文件，必须消毒）。
        name: FieldPath,
        /// `$` 的字节偏移。
        at: usize,
    },

    /// 用了本展开器不支持的 shell 形式。
    ///
    /// **不静默放过**：见模块文档。
    #[error(
        "unsupported substitution `{form}` at byte offset {at}: only `${{NAME}}` and `${{NAME:default}}` are supported"
    )]
    UnsupportedForm {
        /// 消毒过的原文片段。
        form: FieldPath,
        /// `$` 的字节偏移。
        at: usize,
    },

    /// 裸 `$`。
    #[error("stray `$` at byte offset {at}: write `$$` for a literal dollar sign")]
    StrayDollar {
        /// `$` 的字节偏移。
        at: usize,
    },

    /// 变量未定义且没有给默认值。
    #[error("environment variable `{name}` is not set and has no default (at byte offset {at})")]
    Undefined {
        /// 变量名（消毒过）。
        name: FieldPath,
        /// `$` 的字节偏移。
        at: usize,
    },
}

impl ExpandError {
    /// 出错位置的字节偏移。
    #[must_use]
    pub const fn offset(&self) -> usize {
        match self {
            Self::Unterminated { at }
            | Self::InvalidName { at, .. }
            | Self::UnsupportedForm { at, .. }
            | Self::StrayDollar { at }
            | Self::Undefined { at, .. } => *at,
        }
    }
}

/// 在 `input` 上展开变量引用。
///
/// # Errors
///
/// 语法不合法、用了不支持的形式、或变量未定义且无默认值时返回 [`ExpandError`]。
pub fn expand<S: VarSource + ?Sized>(input: &str, vars: &S) -> Result<String, ExpandError> {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // 非 `$` 的部分整段搬运，不逐字符走。
        let Some(dollar) = input[i..].find('$').map(|off| i + off) else {
            out.push_str(&input[i..]);
            break;
        };
        out.push_str(&input[i..dollar]);

        match bytes.get(dollar + 1) {
            // `$$` → 字面 `$`
            Some(b'$') => {
                out.push('$');
                i = dollar + 2;
            }
            Some(b'{') => {
                let body_start = dollar + 2;
                let rel_end = input[body_start..]
                    .find('}')
                    .ok_or(ExpandError::Unterminated { at: dollar })?;
                let body = &input[body_start..body_start + rel_end];
                out.push_str(&resolve(body, dollar, vars)?);
                i = body_start + rel_end + 1;
            }
            // 裸 `$`：不猜，报错。
            _ => return Err(ExpandError::StrayDollar { at: dollar }),
        }
    }

    Ok(out)
}

/// 处理 `${...}` 的花括号内部。
fn resolve<S: VarSource + ?Sized>(body: &str, at: usize, vars: &S) -> Result<String, ExpandError> {
    // 先拦不支持的 shell 形式。`:-` / `:?` / `:+` 都以 `:` 开头，会被下面的 split 切成
    // name + default，默认值恰好以 `-` `?` `+` 开头——那正是运维写错时的样子，必须拦住。
    let (name, default) = match body.split_once(':') {
        Some((name, rest)) => {
            if let Some(first) = rest.chars().next()
                && matches!(first, '-' | '?' | '+' | '=')
            {
                return Err(ExpandError::UnsupportedForm {
                    form: FieldPath::new(&format!("${{{body}}}")),
                    at,
                });
            }
            (name, Some(rest))
        }
        None => (body, None),
    };

    // `${A/x/y}`、`${#A}`、`${!A}` 之类：名字里有非标识符字符。
    if !is_identifier(name) {
        return Err(ExpandError::InvalidName {
            name: FieldPath::new(name),
            at,
        });
    }

    match (vars.get(name), default) {
        (Some(value), _) => Ok(value.to_owned()),
        (None, Some(default)) => Ok(default.to_owned()),
        (None, None) => Err(ExpandError::Undefined {
            name: FieldPath::new(name),
            at,
        }),
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*`，且非空。
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn supported_forms_expand() {
        let env = vars(&[("HOST", "10.0.0.1"), ("EMPTY", "")]);
        let cases: &[(&str, &str, &str)] = &[
            ("no placeholders", "plain text", "plain text"),
            ("bare name", "${HOST}", "10.0.0.1"),
            (
                "embedded",
                "addr = \"${HOST}:8080\"",
                "addr = \"10.0.0.1:8080\"",
            ),
            ("default unused", "${HOST:127.0.0.1}", "10.0.0.1"),
            ("default used", "${MISSING:127.0.0.1}", "127.0.0.1"),
            ("empty default", "${MISSING:}", ""),
            // 存在但为空串 ≠ 不存在：前者是运维显式设成空的，不该被默认值顶掉。
            ("set to empty wins over default", "${EMPTY:fallback}", ""),
            ("two in a row", "${HOST}${HOST}", "10.0.0.110.0.0.1"),
            ("escaped dollar", "pw = \"a$$b\"", "pw = \"a$b\""),
            ("dollar at end", "a$$", "a$"),
            ("default containing colon", "${MISSING:a:b}", "a:b"),
        ];
        for (i, (name, input, expected)) in cases.iter().enumerate() {
            let got =
                expand(input, &env).unwrap_or_else(|e| panic!("TC{i} ({name}) 不该失败: {e}"));
            assert_eq!(&got, expected, "TC{i} ({name}) 展开结果不符");
        }
    }

    #[test]
    fn unsupported_shell_forms_are_rejected_not_passed_through() {
        // 这条用例是这个模块存在的理由。每一个 input 在"支持一部分、其余原样放过"的
        // 实现里都会**静默**变成字面量。
        let env = vars(&[("A", "x")]);
        let cases: &[(&str, &str)] = &[
            ("shell default", "${A:-b}"),
            ("shell error", "${A:?msg}"),
            ("shell alternate", "${A:+b}"),
            ("shell assign", "${A:=b}"),
            ("substring replace", "${A/x/y}"),
            ("length", "${#A}"),
            ("indirect", "${!A}"),
            ("empty name", "${}"),
            ("name with dash", "${A-B}"),
            ("name with space", "${A B}"),
            ("leading digit", "${1A}"),
        ];
        for (i, (name, input)) in cases.iter().enumerate() {
            let err =
                expand(input, &env).expect_err(&format!("TC{i} ({name}) 必须报错，而不是原样放过"));
            assert!(
                !err.to_string().is_empty(),
                "TC{i} ({name}) 的错误必须能说清问题"
            );
        }
    }

    #[test]
    fn stray_dollar_tells_you_to_escape_it() {
        let env = vars(&[]);
        for (i, input) in ["$HOME", "a $ b", "$"].iter().enumerate() {
            let err = expand(input, &env).expect_err(&format!("TC{i} 必须报错"));
            assert!(
                matches!(err, ExpandError::StrayDollar { .. }),
                "TC{i} 应是 StrayDollar，实际 {err:?}"
            );
            assert!(err.to_string().contains("$$"), "TC{i} 的消息要给出转义写法");
        }
    }

    #[test]
    fn undefined_without_default_is_an_error() {
        let env = vars(&[]);
        let err = expand("${MISSING}", &env).expect_err("未定义且无默认值必须报错");
        match err {
            ExpandError::Undefined { ref name, at } => {
                assert_eq!(name.as_str(), "MISSING");
                assert_eq!(at, 0);
            }
            other => panic!("应是 Undefined，实际 {other:?}"),
        }
    }

    #[test]
    fn unterminated_placeholder_reports_the_dollar_offset() {
        let env = vars(&[]);
        let err = expand("port = ${PORT", &env).expect_err("缺 `}` 必须报错");
        assert!(matches!(err, ExpandError::Unterminated { .. }));
        assert_eq!(err.offset(), 7, "偏移必须指向 `$`，运维才能跳到出错的位置");
    }

    #[test]
    fn errors_never_echo_the_variable_value() {
        // 回归用例：即便变量已定义，错误里也只有名字，没有取值。
        let env = vars(&[("SECRET", "hunter2")]);
        let err = expand("${SECRET:-x}", &env).expect_err("不支持的形式必须报错");
        assert!(
            !err.to_string().contains("hunter2"),
            "展开错误不能回显变量取值"
        );
    }
}
