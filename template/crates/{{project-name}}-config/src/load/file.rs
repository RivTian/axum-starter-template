//! The configuration file.

use std::fs;
use std::path::Path;

use toml::{Table, Value};

use super::keys::{join, toml_key};
use super::layered::Layered;
use crate::report::Problem;
use crate::source::Source;

impl Layered {
    /// Reads the file: it may be unreadable or not valid TOML, and every key must be in the
    /// schema. A secret given as an integer or a boolean becomes a string.
    pub(super) fn merge_file(&mut self, path: &Path) {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                let detail = format!("file {} cannot be read: {error}", path.display());
                self.problems.push(Problem::whole(&detail));
                return;
            }
        };
        match toml::from_str::<Table>(&text) {
            Ok(table) => self.merge_table(&table, "", &Source::File(path.to_path_buf())),
            Err(error) => {
                let detail = not_valid_toml(path, &text, &error);
                self.problems.push(Problem::whole(&detail));
            }
        }
    }

    fn merge_table(&mut self, table: &Table, section: &str, source: &Source) {
        for (name, value) in table {
            // A quoted name with a dot is one key in TOML, never a key of the schema: it is
            // shown quoted, as TOML writes it, so it is not taken for the nested key.
            if name.contains('.') {
                let shown = join(section, &toml_key(name));
                self.problems
                    .push(Problem::at(&shown, source, "unknown key"));
                continue;
            }
            let key = join(section, name);
            if self.sections.contains(&key) {
                if let Value::Table(inner) = value {
                    self.merge_table(inner, &key, source);
                } else {
                    let problem = Problem::at(&key, source, "is a section, not a key");
                    self.problems.push(problem);
                }
            } else if self.leaves.contains(&key) {
                let value = match value {
                    Value::Integer(number) if self.secrets.contains(&key) => {
                        Value::String(number.to_string())
                    }
                    Value::Boolean(flag) if self.secrets.contains(&key) => {
                        Value::String(flag.to_string())
                    }
                    _ => value.clone(),
                };
                self.set(&key, value, source.clone());
            } else {
                self.problems.push(Problem::at(&key, source, "unknown key"));
            }
        }
    }
}

/// `file <path> is not valid TOML at line L, column C: <reason>`; without the position when
/// the parser gives none.
fn not_valid_toml(path: &Path, text: &str, error: &toml::de::Error) -> String {
    let at = match error.span() {
        Some(span) => {
            let (line, column) = position(text, span.start);
            format!(" at line {line}, column {column}")
        }
        None => String::new(),
    };
    let reason = error.message();
    format!("file {} is not valid TOML{at}: {reason}", path.display())
}

/// The 1-based line and column (in characters) of a byte offset, as TOML parsers count.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = text.get(..end).unwrap_or_default();
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    (line, column)
}
