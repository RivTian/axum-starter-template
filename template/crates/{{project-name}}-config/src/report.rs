//! The problems found while loading. They are data, not an error: the binary prints them,
//! one line each, and exits.

use std::fmt;

use crate::source::{Source, printable};

/// One problem: about a key and where its value came from, or about the whole input, such as
/// a file that is not valid TOML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The key, such as `server.http_addr`; `None` for a problem of the whole input.
    pub key: Option<String>,
    /// Where the key's value came from; `None` for a problem of the whole input.
    pub source: Option<Source>,
    /// What is wrong, such as `expected a duration like "5s" or "250ms", got "5 seconds"`.
    pub detail: String,
}

impl Problem {
    pub(crate) fn at(key: &str, source: &Source, detail: &str) -> Self {
        Problem {
            key: Some(key.to_string()),
            source: Some(source.clone()),
            detail: detail.to_string(),
        }
    }

    pub(crate) fn whole(detail: &str) -> Self {
        Problem {
            key: None,
            source: None,
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.key, &self.source) {
            (Some(key), Some(source)) => {
                let (key, detail) = (printable(key), &self.detail);
                write!(f, "invalid configuration: {key} ({source}): {detail}")
            }
            _ => write!(f, "invalid configuration: {}", self.detail),
        }
    }
}

/// Every problem found, those of the whole input first, then by key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    problems: Vec<Problem>,
}

impl Report {
    pub(crate) fn new(mut problems: Vec<Problem>) -> Self {
        problems.sort_by(|a, b| a.key.cmp(&b.key));
        Report { problems }
    }

    /// The problems, in order.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, problem) in self.problems.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{problem}")?;
        }
        Ok(())
    }
}
