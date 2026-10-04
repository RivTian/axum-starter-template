//! Keys and values of the TOML tree: dotted keys, how values are shown in problems, and which
//! source wins when values are tried one by one.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use toml::{Table, Value};

use crate::source::Source;

/// The order in which merged values are tried when locating bad keys.
pub(super) fn priority(source: &Source) -> u8 {
    match source {
        Source::Cli(_) => 0,
        Source::Env(_) => 1,
        Source::File(_) | Source::Default => 2,
    }
}

/// The "expected ..." part of serde's messages for a value of the wrong form.
pub(super) fn expected_form(message: &str) -> Option<&str> {
    const KINDS: [&str; 4] = [
        "invalid type: ",
        "invalid value: ",
        "invalid length ",
        "unknown variant ",
    ];
    if !KINDS.iter().any(|kind| message.starts_with(kind)) {
        return None;
    }
    message.rsplit_once(", expected ").map(|(_, form)| form)
}

/// A key as TOML writes it: bare when it can be, otherwise quoted with TOML's escapes.
pub(super) fn toml_key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        return name.to_string();
    }
    let mut quoted = String::from("\"");
    for c in name.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c if c.is_control() => {
                write!(quoted, "\\u{:04X}", u32::from(c)).ok();
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

pub(super) fn join(section: &str, name: &str) -> String {
    if section.is_empty() {
        name.to_string()
    } else {
        format!("{section}.{name}")
    }
}

pub(super) fn collect_keys(
    table: &Table,
    section: &str,
    leaves: &mut BTreeSet<String>,
    sections: &mut BTreeSet<String>,
) {
    for (name, value) in table {
        let key = join(section, name);
        if let Value::Table(inner) = value {
            collect_keys(inner, &key, leaves, sections);
            sections.insert(key);
        } else {
            leaves.insert(key);
        }
    }
}

pub(crate) fn get_value<'t>(table: &'t Table, key: &str) -> Option<&'t Value> {
    let mut parts = key.split('.');
    let first = table.get(parts.next()?)?;
    parts.try_fold(first, |value, part| value.get(part))
}

/// Sets a key whose sections exist, as they do for every key of the schema.
pub(super) fn set_value(table: &mut Table, key: &str, value: Value) {
    let mut current = table;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.to_string(), value);
            return;
        }
        match current.get_mut(part).and_then(Value::as_table_mut) {
            Some(inner) => current = inner,
            None => return,
        }
    }
}

/// A string as shown in a problem: in double quotes, with quotes and control characters
/// escaped.
pub(super) fn quoted(text: &str) -> String {
    format!("{text:?}")
}

/// A value as shown in a problem: strings quoted, other values as written in TOML.
pub(super) fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => quoted(text),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => format!("{number:?}"),
        Value::Boolean(flag) => flag.to_string(),
        Value::Datetime(datetime) => datetime.to_string(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(shown).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Table(table) => {
            let entries: Vec<String> = table
                .iter()
                .map(|(key, value)| format!("{key} = {}", shown(value)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
    }
}
