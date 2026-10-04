//! The defaults with the sources merged in, where each merged value came from, and the
//! problems found on the way.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use toml::{Table, Value};

use super::keys::{collect_keys, expected_form, get_value, priority, set_value, shown};
use crate::report::Problem;
use crate::source::{Source, Sources};

pub(super) struct Layered {
    pub(super) defaults: Table,
    pub(super) value: Table,
    pub(super) leaves: BTreeSet<String>,
    pub(super) sections: BTreeSet<String>,
    pub(super) merged: BTreeMap<String, Source>,
    /// Keys whose value came from an environment variable that is not UTF-8, with the value
    /// as shown in the problem; a later source clears the mark.
    pub(super) not_utf8: BTreeMap<String, String>,
    pub(super) problems: Vec<Problem>,
}

impl Layered {
    pub(super) fn new(defaults: Table) -> Self {
        let mut leaves = BTreeSet::new();
        let mut sections = BTreeSet::new();
        collect_keys(&defaults, "", &mut leaves, &mut sections);
        Layered {
            value: defaults.clone(),
            defaults,
            leaves,
            sections,
            merged: BTreeMap::new(),
            not_utf8: BTreeMap::new(),
            problems: Vec::new(),
        }
    }

    pub(super) fn set(&mut self, key: &str, value: Value, source: Source) {
        set_value(&mut self.value, key, value);
        self.merged.insert(key.to_string(), source);
        self.not_utf8.remove(key);
    }

    pub(super) fn merge_cli(&mut self, cli: &[(&'static str, &'static str, String)]) {
        for (key, flag, value) in cli {
            let source = Source::Cli(flag);
            if self.leaves.contains(*key) {
                self.set(key, Value::String(value.clone()), source);
            } else {
                self.problems.push(Problem::at(key, &source, "unknown key"));
            }
        }
    }

    /// The whole tree did not deserialize. Each merged value is put alone into the defaults,
    /// command line first, then environment, then file; every value that fails on its own is
    /// a bad key with its source.
    pub(super) fn locate_bad_keys<T: DeserializeOwned>(&mut self, whole: &toml::de::Error) {
        let reported: BTreeSet<String> = self
            .problems
            .iter()
            .filter_map(|problem| problem.key.clone())
            .collect();
        let mut candidates: Vec<(&String, &Source)> = (self.merged.iter())
            .filter(|(key, _)| !reported.contains(*key))
            .collect();
        candidates.sort_by_key(|(key, source)| (priority(source), *key));
        let mut found = Vec::new();
        for (key, source) in candidates {
            if let Some(value) = get_value(&self.value, key)
                && let Some(detail) = self.fails_alone::<T>(key, value)
            {
                found.push(Problem::at(key, source, &detail));
            }
        }
        if found.is_empty() && self.problems.is_empty() {
            found.push(Problem::whole(whole.message()));
        }
        self.problems.extend(found);
    }

    /// Why the value fails when it is the only one set, or `None` if it does not. For an
    /// array key, the first element that fails on its own is named.
    fn fails_alone<T: DeserializeOwned>(&self, key: &str, value: &Value) -> Option<String> {
        let error = self.try_alone::<T>(key, value.clone()).err()?;
        if let (Some(Value::Array(_)), Value::Array(items)) =
            (get_value(&self.defaults, key), value)
        {
            for (index, item) in items.iter().enumerate() {
                if let Err(element) = self.try_alone::<T>(key, Value::Array(vec![item.clone()])) {
                    let detail = Self::detail(item, &element);
                    return Some(format!("{detail} (element {index})"));
                }
            }
        }
        Some(Self::detail(value, &error))
    }

    fn try_alone<T: DeserializeOwned>(
        &self,
        key: &str,
        value: Value,
    ) -> Result<T, toml::de::Error> {
        let mut tree = self.defaults.clone();
        set_value(&mut tree, key, value);
        T::deserialize(Value::Table(tree))
    }

    /// `expected <form>, got <value>` for a value of the wrong form, with the form from the
    /// field type and the value as given; otherwise the field's
    /// own message, such as `must be between 1 and 65536, got 0`.
    fn detail(value: &Value, error: &toml::de::Error) -> String {
        let message = error.message();
        match expected_form(message) {
            Some(form) => format!("expected {form}, got {}", shown(value)),
            None => message.to_string(),
        }
    }

    pub(super) fn sources(&self) -> Sources {
        let mut sources = Sources::default();
        for key in &self.leaves {
            let source = self.merged.get(key).cloned().unwrap_or(Source::Default);
            sources.set(key, source);
        }
        sources
    }
}
