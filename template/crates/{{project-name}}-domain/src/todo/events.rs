//! What happened to a todo, announced after the change is stored.

use super::TodoId;

/// Something that happened to a todo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TodoEvent {
    /// A todo was created.
    Created {
        /// The todo.
        id: TodoId,
    },
    /// A todo was completed.
    Completed {
        /// The todo.
        id: TodoId,
    },
    /// A todo was deleted.
    Deleted {
        /// The todo.
        id: TodoId,
    },
}

impl TodoEvent {
    /// The name, as logged in `event.name`: `todo_created`, `todo_completed` or
    /// `todo_deleted`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            TodoEvent::Created { .. } => "todo_created",
            TodoEvent::Completed { .. } => "todo_completed",
            TodoEvent::Deleted { .. } => "todo_deleted",
        }
    }

    /// The todo the event is about.
    #[must_use]
    pub const fn id(&self) -> TodoId {
        match self {
            TodoEvent::Created { id } | TodoEvent::Completed { id } | TodoEvent::Deleted { id } => {
                *id
            }
        }
    }
}
