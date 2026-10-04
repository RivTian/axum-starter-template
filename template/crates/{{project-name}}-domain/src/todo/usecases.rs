//! The todo use cases: creating, reading, completing and deleting todos. They say what went
//! wrong with an error kind; they do not decide HTTP status codes.

use std::sync::Arc;

use svc_util::prelude::*;

use super::{
    NewTodo, TODO_NOT_FOUND, TODO_VERSION_CONFLICT, Title, Todo, TodoEvent, TodoId, TodoPublisher,
    TodoRepository, UpdateOutcome,
};
use crate::clock::Clock;

/// The todo use cases, over injected ports.
#[derive(Clone)]
pub struct TodoUseCases {
    repository: Arc<dyn TodoRepository>,
    publisher: Arc<dyn TodoPublisher>,
    clock: Arc<dyn Clock>,
}

impl TodoUseCases {
    /// The use cases over a repository, a publisher and a clock.
    #[must_use]
    pub fn new(
        repository: Arc<dyn TodoRepository>,
        publisher: Arc<dyn TodoPublisher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        TodoUseCases {
            repository,
            publisher,
            clock,
        }
    }

    /// Creates a todo with this title.
    ///
    /// # Errors
    ///
    /// [`INVALID_TODO_TITLE`](super::INVALID_TODO_TITLE) for a bad title; a repository error.
    pub async fn create(&self, title: &str) -> Result<Todo> {
        let title = Title::new(title)?;
        let id = TodoId::at(self.clock.now());
        let todo = self.repository.insert(NewTodo { id, title }).await?;
        self.publisher.publish(TodoEvent::Created { id: todo.id });
        Ok(todo)
    }

    /// The todo with this id.
    ///
    /// # Errors
    ///
    /// [`TODO_NOT_FOUND`]; a repository error.
    pub async fn get(&self, id: TodoId) -> Result<Todo> {
        self.repository
            .get(id)
            .await?
            .or_err_with(TODO_NOT_FOUND, || not_found(id))
    }

    /// At most `limit` todos, oldest first.
    ///
    /// # Errors
    ///
    /// A repository error.
    pub async fn list(&self, limit: usize) -> Result<Vec<Todo>> {
        self.repository.list(limit).await
    }

    /// Marks the todo done, if the caller has seen its current version.
    ///
    /// # Errors
    ///
    /// [`TODO_NOT_FOUND`]; [`TODO_VERSION_CONFLICT`] when the todo is at another version than
    /// `expected`; a repository error.
    pub async fn complete(&self, id: TodoId, expected: u64) -> Result<Todo> {
        let mut todo = self.get(id).await?;
        if todo.version != expected {
            return conflict(expected, todo.version);
        }
        todo.done = true;
        match self.repository.update(&todo, expected).await? {
            UpdateOutcome::Updated(todo) => {
                self.publisher.publish(TodoEvent::Completed { id });
                Ok(todo)
            }
            UpdateOutcome::VersionMismatch { current } => conflict(expected, current),
            UpdateOutcome::NotFound => Error::e_explain(TODO_NOT_FOUND, not_found(id)),
        }
    }

    /// Deletes the todo.
    ///
    /// # Errors
    ///
    /// [`TODO_NOT_FOUND`]; a repository error.
    pub async fn delete(&self, id: TodoId) -> Result<()> {
        if !self.repository.delete(id).await? {
            return Error::e_explain(TODO_NOT_FOUND, not_found(id));
        }
        self.publisher.publish(TodoEvent::Deleted { id });
        Ok(())
    }
}

fn not_found(id: TodoId) -> String {
    format!("todo {id} does not exist")
}

fn conflict<T>(expected: u64, current: u64) -> Result<T> {
    Error::e_explain(
        TODO_VERSION_CONFLICT,
        format!("expected version {expected}, found {current}"),
    )
}
