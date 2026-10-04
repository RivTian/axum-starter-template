//! Deserializers for configuration fields: they check form and range while the configuration
//! is read, so a bad value is reported with its key and source before anything starts. Use
//! them with `#[serde(deserialize_with = "svc_util::de::duration::<_, 1, 300>")]`.
//!
//! A value of the wrong form fails with serde's `invalid type` or `invalid value` error,
//! whose "expected ..." text comes from the visitor's `expecting`; the loader adds the
//! value it was given. A value of the right form but outside the range fails with a
//! complete message such as `must be between 1 and 65536, got 0`.

use std::fmt;
use std::net::SocketAddr;
use std::num::IntErrorKind;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::{self, Deserializer, Unexpected, Visitor};

use crate::duration;

/// A duration from `MIN` to `MAX` seconds, written like `"5s"` or `"250ms"`.
///
/// # Errors
///
/// When the value is not a duration, or is out of range.
pub fn duration<'de, D: Deserializer<'de>, const MIN: u64, const MAX: u64>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    struct DurationVisitor<const MIN: u64, const MAX: u64>;

    impl<const MIN: u64, const MAX: u64> Visitor<'_> for DurationVisitor<MIN, MAX> {
        type Value = Duration;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(duration::EXPECTED)
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<Duration, E> {
            let range = || {
                let range = if MIN == 0 {
                    format!("at most {MAX}s")
                } else {
                    format!("between {MIN}s and {MAX}s")
                };
                E::custom(format!("must be {range}, got {}", quoted(text)))
            };
            let Some(value) = duration::parse(text) else {
                // The right form with a number too large for the machine is out of range.
                return Err(if has_duration_form(text) {
                    range()
                } else {
                    E::invalid_value(Unexpected::Str(text), &self)
                });
            };
            if value < Duration::from_secs(MIN) || value > Duration::from_secs(MAX) {
                return Err(range());
            }
            Ok(value)
        }
    }

    deserializer.deserialize_any(DurationVisitor::<MIN, MAX>)
}

/// Whether the text is `<digits><unit>` as `svc_util::duration::parse` takes it, whatever
/// the size of the number.
fn has_duration_form(text: &str) -> bool {
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    digits > 0 && matches!(&text[digits..], "h" | "m" | "s" | "ms")
}

/// An integer from `MIN` to `MAX`. A quoted integer counts too: environment variables and
/// command-line flags give every value as text.
///
/// # Errors
///
/// When the value is not an integer, or is out of range.
pub fn integer<'de, D: Deserializer<'de>, const MIN: u64, const MAX: u64>(
    deserializer: D,
) -> Result<usize, D::Error> {
    struct IntegerVisitor<const MIN: u64, const MAX: u64>;

    impl<const MIN: u64, const MAX: u64> IntegerVisitor<MIN, MAX> {
        fn in_range<E: de::Error>(
            value: Option<u64>,
            shown: &dyn fmt::Display,
        ) -> Result<usize, E> {
            value
                .filter(|value| (MIN..=MAX).contains(value))
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| E::custom(format!("must be between {MIN} and {MAX}, got {shown}")))
        }
    }

    impl<const MIN: u64, const MAX: u64> Visitor<'_> for IntegerVisitor<MIN, MAX> {
        type Value = usize;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an integer between {MIN} and {MAX}")
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<usize, E> {
            Self::in_range(u64::try_from(value).ok(), &value)
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<usize, E> {
            Self::in_range(Some(value), &value)
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<usize, E> {
            match text.parse::<i64>() {
                Ok(value) => Self::in_range(u64::try_from(value).ok(), &quoted(text)),
                // An integer too large for the machine is out of range, not of the wrong form.
                Err(error)
                    if matches!(
                        error.kind(),
                        IntErrorKind::PosOverflow | IntErrorKind::NegOverflow
                    ) =>
                {
                    Self::in_range(None, &quoted(text))
                }
                Err(_) => Err(E::invalid_value(Unexpected::Str(text), &self)),
            }
        }
    }

    deserializer.deserialize_any(IntegerVisitor::<MIN, MAX>)
}

/// `true` or `false`; environment variables and command-line flags give the words as text.
///
/// # Errors
///
/// When the value is not `true` or `false`.
pub fn boolean<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    struct BooleanVisitor;

    impl Visitor<'_> for BooleanVisitor {
        type Value = bool;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("true or false")
        }

        fn visit_bool<E: de::Error>(self, value: bool) -> Result<bool, E> {
            Ok(value)
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<bool, E> {
            match text {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(E::invalid_value(Unexpected::Str(text), &self)),
            }
        }
    }

    deserializer.deserialize_any(BooleanVisitor)
}

/// A socket address such as `127.0.0.1:8080`. Host names are not resolved.
///
/// # Errors
///
/// When the value is not a socket address.
pub fn socket_addr<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SocketAddr, D::Error> {
    struct SocketAddrVisitor;

    impl Visitor<'_> for SocketAddrVisitor {
        type Value = SocketAddr;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(r#"a socket address like "127.0.0.1:8080""#)
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<SocketAddr, E> {
            text.parse()
                .map_err(|_| E::invalid_value(Unexpected::Str(text), &self))
        }
    }

    deserializer.deserialize_any(SocketAddrVisitor)
}

/// A path that is not empty.
///
/// # Errors
///
/// When the value is not a string, or is empty.
pub fn non_empty_path<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    struct PathVisitor;

    impl Visitor<'_> for PathVisitor {
        type Value = PathBuf;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<PathBuf, E> {
            if text.is_empty() {
                return Err(E::custom("must not be empty"));
            }
            Ok(PathBuf::from(text))
        }
    }

    deserializer.deserialize_any(PathVisitor)
}

/// Text that is not empty.
///
/// # Errors
///
/// When the value is not a string, or is empty.
pub fn non_empty_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    struct TextVisitor;

    impl Visitor<'_> for TextVisitor {
        type Value = String;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<String, E> {
            if text.is_empty() {
                return Err(E::custom("must not be empty"));
            }
            Ok(text.to_string())
        }
    }

    deserializer.deserialize_any(TextVisitor)
}

/// One of the given words, each standing for a value; the error lists every word.
///
/// # Errors
///
/// When the value is not one of the words.
pub fn one_of<'de, D: Deserializer<'de>, T: Copy + 'static>(
    deserializer: D,
    choices: &'static [(&'static str, T)],
) -> Result<T, D::Error> {
    struct OneOfVisitor<T: 'static>(&'static [(&'static str, T)]);

    impl<T: Copy> Visitor<'_> for OneOfVisitor<T> {
        type Value = T;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("one of ")?;
            for (index, (word, _)) in self.0.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{}", quoted(word))?;
            }
            Ok(())
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<T, E> {
            self.0
                .iter()
                .find(|(word, _)| *word == text)
                .map(|(_, value)| *value)
                .ok_or_else(|| E::invalid_value(Unexpected::Str(text), &self))
        }
    }

    deserializer.deserialize_any(OneOfVisitor(choices))
}

/// A string as shown in an error: in double quotes, with quotes and control characters
/// escaped.
fn quoted(text: &str) -> String {
    format!("{text:?}")
}
