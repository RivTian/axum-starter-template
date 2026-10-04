//! Environment variables: aliases such as `RUST_LOG`, and `<PREFIX>_<SECTION>__<KEY>`.

use std::ffi::{OsStr, OsString};

use svc_util::secret::REDACTED;
use toml::Value;

use super::keys::quoted;
use super::layered::Layered;
use crate::report::Problem;
use crate::source::Source;

impl Layered {
    /// Sets a key from an environment variable. A value that is not UTF-8 is kept with U+FFFD
    /// in place of the bad bytes and marked, so that it is reported unless a later source
    /// replaces it.
    fn set_text(&mut self, key: &str, value: &OsStr, source: Source) {
        let text = value.to_string_lossy().into_owned();
        self.set(key, Value::String(text.clone()), source);
        if value.to_str().is_none() {
            let shown = if self.secrets.contains(key) {
                REDACTED.to_string()
            } else {
                quoted(&text)
            };
            self.not_utf8.insert(key.to_string(), shown);
        }
    }

    /// Each alias sets its key, below the project's own variables. An empty value, as
    /// `RUST_LOG=${RUST_LOG}` in a compose file gives, counts as unset.
    pub(super) fn merge_aliases(
        &mut self,
        aliases: &[(&'static str, &'static str)],
        env: &[(OsString, OsString)],
    ) {
        for (key, variable) in aliases {
            if !self.leaves.contains(*key) {
                continue;
            }
            let set = env
                .iter()
                .find(|(name, value)| name == variable && !value.is_empty());
            if let Some((_, value)) = set {
                self.set_text(key, value, Source::Env((*variable).to_string()));
            }
        }
    }

    /// `<PREFIX>_<SECTION>__<KEY>` sets `section.key`; the value is always text, which each
    /// field type parses. A prefixed name without `__` after the prefix is not a key
    /// (`<PREFIX>_CONFIG` names the file), so it is left alone.
    pub(super) fn merge_env(&mut self, prefix: &str, env: &[(OsString, OsString)]) {
        let start = format!("{prefix}_");
        // A name that is not UTF-8 is shown with U+FFFD; it can only be an unknown key.
        let mut vars: Vec<(String, &OsString)> = (env.iter())
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value))
            .filter(|(name, _)| {
                name.strip_prefix(&start)
                    .is_some_and(|rest| rest.contains("__"))
            })
            .collect();
        vars.sort();
        for (name, value) in vars {
            let key = name[start.len()..].to_ascii_lowercase().replace("__", ".");
            let source = Source::Env(name.clone());
            if self.leaves.contains(&key) {
                self.set_text(&key, value, source);
            } else if self.sections.contains(&key) {
                self.problems
                    .push(Problem::at(&key, &source, "is a section, not a key"));
            } else {
                self.problems
                    .push(Problem::at(&key, &source, "unknown key"));
            }
        }
    }

    /// Environment values that are not UTF-8 and still take effect.
    pub(super) fn report_not_utf8(&mut self) {
        for (key, shown) in std::mem::take(&mut self.not_utf8) {
            let source = self.merged.get(&key).cloned().unwrap_or(Source::Default);
            let detail = format!("expected UTF-8 text, got {shown}");
            self.problems.push(Problem::at(&key, &source, &detail));
        }
    }
}
