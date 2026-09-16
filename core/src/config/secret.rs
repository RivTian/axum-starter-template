//! 敏感取值：**`Debug` 渲染成 `***`，来源按 `env > file > 明文` 取。**
//!
//! # 模板自己不用它
//!
//! 说在最前面：本模板的 [`Config`](super::Config) **没有任何敏感字段**——唯一的后端是
//! SQLite，它不带凭据（内置的那几项能力里也没有需要凭据的）。所以这个模块在模板里
//! **零调用点**。
//!
//! 这与「不预铺没有消费者的东西」相抵触，因此需要给出理由，而不是默默留着：
//!
//! 1. **它是「错误里不带取值」这条纪律的执行机构，不是一个猜出来的形状。** `error.rs` 里
//!    [`SafeMessage`](super::SafeMessage) 的安全契约明确写着：serde 在类型不符时可能把
//!    原值放进错误消息，因此**可能承载敏感内容的字段必须是 `Secret`**。没有这个类型，
//!    那条契约就成了一句没有执行途径的规定——文档写了，代码里无从遵守。
//! 2. **它的形状是被指定的，不是被推测的。** 优先级阶梯与 `***` 渲染都是照着规格写的；
//!    被禁的是「预铺猜测出来的形状」，而这里没有需要猜的部分。
//!
//! 反过来说，能省的确实省了：这里**没有**"擦除内存"（`zeroize` 之类）。那需要一个额外
//! 依赖，而它在一个跑在 GC 之外、还会把配置整体 `Clone` 进 watch 通道的进程里给不出
//! 它承诺的保证——那才是"为了样子加依赖"。
//!
//! # 读取在别处
//!
//! [`SecretSource::resolve`] 不读环境、不读文件：它收一个 [`SecretLookup`]。真实的
//! 环境与文件访问在 `app`（进程边界只有一处）。于是优先级阶梯这段**判断**可以在
//! 内存里被完整遍历，而不需要摆弄真实的环境变量。

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use super::error::{ConfigError, FieldPath};

/// 一个不会被打印出来的取值。
///
/// 没有 `Display`：想把它渲染出来，只能显式走 [`Secret::expose`]，而那个名字在
/// `git grep expose` 里一眼就能看见。这是它与 `String` 的**全部**区别，也是全部价值。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// 包起来。
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// 取出内层取值。
    ///
    /// 名字刻意难看。每一处调用都是一次"这里真的需要明文吗"的提问，而 `grep` 能把
    /// 这些提问全部列出来——`.0` 或 `Deref` 做不到这件事。
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// 消耗自身取出内层取值。
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    /// 永远是 `***`，与 `T` 无关。
    ///
    /// 不打 `Secret(***)` 之类带类型名的形式：`{:#?}` 一整个 `Config` 时，`***` 已经
    /// 足够说明这是被遮住的，多出来的类型名只是噪音。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<'de, T> Deserialize<'de> for Secret<T>
where
    T: Deserialize<'de>,
{
    /// 手写而非 `derive`。
    ///
    /// `derive` 出来的实现会把 `T` 的反序列化错误原样上抛，而 serde 的错误在类型不符时
    /// 会带上**原值**（`invalid type: string "hunter2", expected u16`）。对一个敏感字段
    /// 来说，那正是最不该发生的事：配错类型的那一刻，口令进了日志。
    ///
    /// 这里把错误整个换掉，只留下"值的形状不对"这一个信息。定位靠
    /// `serde_path_to_error` 在外层记的路径，不靠错误消息里的内容。
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self).map_err(|_| {
            serde::de::Error::custom("secret value has the wrong shape (value withheld)")
        })
    }
}

/// 敏感取值的三段来源。
///
/// ```toml
/// # 三选一，按 env > file > 明文 取第一个给出的
/// [some_credential]
/// env = "MY_SERVICE_DB_PASSWORD"
/// # file = "/run/secrets/db_password"
/// # value = "只在本地开发时用"
/// ```
///
/// 为什么是"取第一个给出的"而不是"取第一个成功的"：`env` 写了但变量没设，是**配置错误**，
/// 不是"那就退回去读文件吧"。后者会让一次部署疏漏（忘了注入变量）静默降级成读到一份
/// 过期的本地文件，而且启动日志上看不出任何异常。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SecretSource {
    /// 环境变量名。
    pub env: Option<String>,
    /// 文件路径。整个文件内容即取值（尾部空白会被剪掉）。
    pub file: Option<PathBuf>,
    /// 明文。开发期便利，不该出现在生产配置里。
    pub value: Option<Secret<String>>,
}

/// 环境与文件的读取途径。
///
/// 两个方法都返回 `Option`：`core` 不需要知道"文件为什么读不出来"，`app` 那一侧在返回
/// `None` 之前会把真实的 io 错误记下来。这样既保住了"进程边界只有一处"，
/// 又不让 io 错误类型爬进 `core` 的公共 API。
pub trait SecretLookup {
    /// 读环境变量。
    fn read_env(&self, name: &str) -> Option<String>;
    /// 读文件内容。
    fn read_file(&self, path: &Path) -> Option<String>;
}

impl SecretSource {
    /// 按 `env > file > 明文` 解析。
    ///
    /// `field` 是这个字段在配置里的路径，只用于错误定位。
    ///
    /// # Errors
    ///
    /// 三段都没写，或写了的那一段取不到内容时，返回 [`ConfigError::Secret`]。
    pub fn resolve<L: SecretLookup + ?Sized>(
        &self,
        field: &str,
        lookup: &L,
    ) -> Result<Secret<String>, ConfigError> {
        // 无 `_` 臂的意图：新增一段来源时，这里必须显式回答它排在哪一档。
        let (raw, tried) = match (&self.env, &self.file, &self.value) {
            (Some(name), _, _) => (lookup.read_env(name), "env"),
            (None, Some(path), _) => (
                lookup.read_file(path).map(|s| s.trim_end().to_owned()),
                "file",
            ),
            (None, None, Some(value)) => (Some(value.expose().clone()), "value"),
            (None, None, None) => (None, "nothing"),
        };

        raw.map(Secret::new).ok_or(ConfigError::Secret {
            path: FieldPath::new(field),
            tried,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct FakeLookup {
        env: BTreeMap<String, String>,
        files: BTreeMap<PathBuf, String>,
    }

    impl SecretLookup for FakeLookup {
        fn read_env(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        fn read_file(&self, path: &Path) -> Option<String> {
            self.files.get(path).cloned()
        }
    }

    fn lookup() -> FakeLookup {
        FakeLookup {
            env: [("DB_PASSWORD".to_owned(), "from-env".to_owned())]
                .into_iter()
                .collect(),
            files: [(PathBuf::from("/run/secrets/db"), "from-file\n".to_owned())]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn secret_debug_never_prints_the_value() {
        // 规格点名的用例。
        let secret = Secret::new("hunter2".to_owned());
        for (i, rendered) in [format!("{secret:?}"), format!("{secret:#?}")]
            .iter()
            .enumerate()
        {
            assert!(
                !rendered.contains("hunter2"),
                "TC{i} 泄漏了取值: {rendered}"
            );
            assert_eq!(rendered, "***", "TC{i} 的遮蔽形式必须稳定");
        }
    }

    #[test]
    fn a_secret_nested_in_a_struct_is_still_redacted() {
        // 真实的泄漏路径不是直接打印 Secret，而是 `{:#?}` 一整个 Config。
        let source = SecretSource {
            env: None,
            file: None,
            value: Some(Secret::new("hunter2".to_owned())),
        };
        let rendered = format!("{source:#?}");
        assert!(
            !rendered.contains("hunter2"),
            "嵌套渲染泄漏了取值: {rendered}"
        );
        assert!(rendered.contains("***"));
    }

    #[test]
    fn sources_are_tried_in_the_declared_priority() {
        let lookup = lookup();
        let cases: &[(&str, SecretSource, &str)] = &[
            (
                "env wins over everything",
                SecretSource {
                    env: Some("DB_PASSWORD".to_owned()),
                    file: Some(PathBuf::from("/run/secrets/db")),
                    value: Some(Secret::new("inline".to_owned())),
                },
                "from-env",
            ),
            (
                "file wins over inline",
                SecretSource {
                    env: None,
                    file: Some(PathBuf::from("/run/secrets/db")),
                    value: Some(Secret::new("inline".to_owned())),
                },
                "from-file",
            ),
            (
                "inline is the last resort",
                SecretSource {
                    env: None,
                    file: None,
                    value: Some(Secret::new("inline".to_owned())),
                },
                "inline",
            ),
        ];
        for (i, (name, source, expected)) in cases.iter().enumerate() {
            let got = source
                .resolve("db.password", &lookup)
                .unwrap_or_else(|e| panic!("TC{i} ({name}) 不该失败: {e}"));
            assert_eq!(got.expose(), expected, "TC{i} ({name}) 取到的来源不对");
        }
    }

    #[test]
    fn a_declared_source_that_yields_nothing_is_an_error_not_a_fallback() {
        // 这条守的是"忘了注入变量"不会静默降级成读一份过期的本地文件。
        let lookup = lookup();
        let source = SecretSource {
            env: Some("NOT_INJECTED".to_owned()),
            file: Some(PathBuf::from("/run/secrets/db")),
            value: Some(Secret::new("inline".to_owned())),
        };
        let err = source
            .resolve("db.password", &lookup)
            .expect_err("写了 env 却没设变量必须报错，而不是退回 file");
        assert!(matches!(err, ConfigError::Secret { tried: "env", .. }));
    }

    #[test]
    fn an_empty_source_block_is_an_error() {
        let err = SecretSource::default()
            .resolve("db.password", &lookup())
            .expect_err("三段都没写必须报错");
        assert_eq!(err.kind_str(), "secret");
        assert!(err.to_string().contains("db.password"));
    }

    #[test]
    fn file_contents_lose_their_trailing_newline() {
        // `printf 'pw' > f` 与 `echo pw > f` 不该产生两个不同的口令。
        let got = SecretSource {
            env: None,
            file: Some(PathBuf::from("/run/secrets/db")),
            value: None,
        }
        .resolve("db.password", &lookup())
        .unwrap_or_else(|e| panic!("不该失败: {e}"));
        assert_eq!(got.expose(), "from-file");
    }

    /// 一个最小的自描述输入源。
    ///
    /// 用 serde 自带的 `value::StrDeserializer` 而不是引一个 `serde_json` dev-dependency：
    /// 这里要验的是"错误消息里有没有取值"，它与格式无关，多一个依赖只是多一个版本要盯。
    fn from_str_input<T>(input: &'static str) -> Result<T, serde::de::value::Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        T::deserialize(serde::de::value::StrDeserializer::new(input))
    }

    #[test]
    fn secret_deserialize_error_never_contains_the_value() {
        // serde 的默认消息是 `invalid type: string "hunter2", expected u16`。
        // 手写的实现把它整个换掉——否则配错类型的那一刻口令就进了日志。
        let err = from_str_input::<Secret<u16>>("hunter2").expect_err("类型不符必须失败");
        let rendered = err.to_string();
        assert!(
            !rendered.contains("hunter2"),
            "反序列化错误泄漏了取值: {rendered}"
        );
        assert!(rendered.contains("withheld"));
    }

    #[test]
    fn the_unprotected_form_would_have_leaked_it() {
        // 对照组：同样的输入直接给 u16，serde 的消息里就有取值。
        // 没有这一半，上面那条用例无法说明问题——它可能只是因为 u16 的错误本来就不带值。
        let err = from_str_input::<u16>("hunter2").expect_err("类型不符必须失败");
        assert!(
            err.to_string().contains("hunter2"),
            "前提不成立：serde 本身没有回显取值，那么 Secret 的手写实现就没在解决问题"
        );
    }

    #[test]
    fn a_well_formed_secret_still_deserializes() {
        let got = from_str_input::<Secret<String>>("hunter2")
            .unwrap_or_else(|e| panic!("合法取值不该失败: {e}"));
        assert_eq!(got.expose(), "hunter2");
    }
}
