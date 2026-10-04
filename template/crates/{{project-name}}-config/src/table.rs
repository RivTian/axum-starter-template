//! The effective configuration as people read it: every key with its value and source, for
//! `check-config`, and the keys that differ from their defaults, for the startup log.

use std::fmt::Write as _;

use serde::Serialize;
use toml::Value;

use crate::load::Loaded;
use crate::source::{Source, printable};

/// One key: its name, its value as shown, and where the value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The key, such as `server.http_addr`.
    pub key: String,
    /// The value as shown: text as it is, other values as TOML writes them; an empty text or
    /// one with control characters is quoted and escaped, so that each key keeps one line.
    pub value: String,
    /// Where the value came from.
    pub source: Source,
}

/// Every key, in key order.
#[must_use]
pub fn rows<T: Serialize>(loaded: &Loaded<T>) -> Vec<Row> {
    let values = Value::try_from(&loaded.config).ok();
    (loaded.sources.iter())
        .map(|(key, source)| Row {
            key: printable(key),
            value: shown_at(values.as_ref(), key),
            source: source.clone(),
        })
        .collect()
}

/// The rows under a header, in columns two spaces apart; widths count characters, as the
/// padding does.
#[must_use]
pub fn render(rows: &[Row]) -> String {
    let width = |cell: fn(&Row) -> &str, header: &str| {
        let widest = rows.iter().map(|row| cell(row).chars().count());
        widest.chain([header.chars().count()]).max().unwrap_or(0)
    };
    let key_width = width(|row| &row.key, "key");
    let value_width = width(|row| &row.value, "value");
    let mut out = format!("{:key_width$}  {:value_width$}  source\n", "key", "value");
    for Row { key, value, source } in rows {
        writeln!(out, "{key:key_width$}  {value:value_width$}  {source}").ok();
    }
    out
}

/// The keys whose effective value differs from their default, as `key=value (source)` joined
/// by `, `, or `none`. A file that repeats a default changes nothing and is not listed.
#[must_use]
pub fn overrides<T: Serialize + Default>(loaded: &Loaded<T>) -> String {
    let (values, defaults) = (
        Value::try_from(&loaded.config).ok(),
        Value::try_from(T::default()).ok(),
    );
    let changed: Vec<String> = (loaded.sources.iter())
        .filter_map(|(key, source)| {
            let value = shown_at(values.as_ref(), key);
            (value != shown_at(defaults.as_ref(), key))
                .then(|| format!("{}={value} ({source})", printable(key)))
        })
        .collect();
    if changed.is_empty() {
        "none".to_string()
    } else {
        changed.join(", ")
    }
}

/// The configuration file that was read, as shown on one line, or `none`.
#[must_use]
pub fn file<T>(loaded: &Loaded<T>) -> String {
    (loaded.file.as_deref()).map_or_else(
        || "none".to_string(),
        |path| printable(&path.display().to_string()),
    )
}

/// The value at a dotted key, as shown; empty when there is none.
fn shown_at(values: Option<&Value>, key: &str) -> String {
    let mut parts = key.split('.');
    let first = parts.next().and_then(|part| values?.get(part));
    let value = first.and_then(|first| parts.try_fold(first, |value, part| value.get(part)));
    value.map(shown).unwrap_or_default()
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(text) if text.is_empty() || text.chars().any(char::is_control) => {
            toml_string(text)
        }
        Value::String(text) => text.clone(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => number.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        Value::Datetime(datetime) => datetime.to_string(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(shown).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Table(_) => String::new(),
    }
}

/// A TOML basic string: quoted, with quotes, backslashes and control characters escaped.
fn toml_string(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if c.is_control() => {
                write!(quoted, "\\u{:04X}", u32::from(c)).ok();
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}
