//! The todo, its id and its title.

use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;

use svc_util::prelude::*;
use time::OffsetDateTime;
use uuid::{ContextV7, Timestamp, Uuid};

use super::INVALID_TODO_TITLE;

/// Keeps ids made in the same millisecond in the order they were made.
static ID_CONTEXT: Mutex<ContextV7> = Mutex::new(ContextV7::new());

/// The id of a todo: a `UUIDv7` carrying the creation time, so ids sort in creation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TodoId(Uuid);

impl TodoId {
    /// The id for a todo created at `at`.
    #[must_use]
    pub fn at(at: OffsetDateTime) -> Self {
        let seconds = u64::try_from(at.unix_timestamp()).unwrap_or(0);
        TodoId(Uuid::new_v7(Timestamp::from_unix(
            &ID_CONTEXT,
            seconds,
            at.nanosecond(),
        )))
    }
}

impl fmt::Display for TodoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for TodoId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Uuid::parse_str(text).map(TodoId)
    }
}

/// A todo's title: 1 to 200 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Title(String);

impl Title {
    /// The longest title, in Unicode characters.
    pub const MAX_CHARS: usize = 200;

    /// A title from text.
    ///
    /// # Errors
    ///
    /// [`INVALID_TODO_TITLE`] when the text is empty or longer than [`Title::MAX_CHARS`].
    pub fn new(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        if text.is_empty() {
            return Error::e_explain(INVALID_TODO_TITLE, "todo title must not be empty");
        }
        if text.chars().count() > Self::MAX_CHARS {
            return Error::e_explain(
                INVALID_TODO_TITLE,
                "todo title must have at most 200 characters",
            );
        }
        Ok(Title(text))
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A todo. `version` starts at 1 and grows by one with every change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    /// The id.
    pub id: TodoId,
    /// The title.
    pub title: Title,
    /// Whether it is done.
    pub done: bool,
    /// The version, for optimistic concurrency.
    pub version: u64,
}

/// What a repository stores for a new todo: not done, version 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTodo {
    /// The id.
    pub id: TodoId,
    /// The title.
    pub title: Title,
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::{Title, TodoId};

    fn rejection(text: &str) -> Option<String> {
        Title::new(text).err().map(|error| {
            let fields = error.fields();
            format!("{}: {}", fields.etype, fields.context)
        })
    }

    #[test]
    fn titles_have_one_to_two_hundred_characters() {
        assert_eq!(
            Title::new("Buy milk").map(|t| t.as_str().to_string()).ok(),
            Some("Buy milk".to_string())
        );
        assert_eq!(
            rejection("").as_deref(),
            Some("InvalidTodoTitle: todo title must not be empty")
        );
        assert!(Title::new("x".repeat(200)).is_ok());
        assert_eq!(
            rejection(&"x".repeat(201)).as_deref(),
            Some("InvalidTodoTitle: todo title must have at most 200 characters")
        );
        // Characters, not bytes: 200 three-byte characters are fine.
        assert!(Title::new("\u{6c34}".repeat(200)).is_ok());
    }

    #[test]
    fn ids_sort_in_creation_order_and_round_trip_as_text() {
        let first = TodoId::at(datetime!(2026-09-30 12:00 UTC));
        let same_millisecond = TodoId::at(datetime!(2026-09-30 12:00 UTC));
        let later = TodoId::at(datetime!(2026-09-30 12:00:01 UTC));
        assert!(first < same_millisecond && same_millisecond < later);
        assert_eq!(first.to_string().parse::<TodoId>().ok(), Some(first));
        assert!("not-a-uuid".parse::<TodoId>().is_err());
    }
}
