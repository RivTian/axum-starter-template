//! What the todo use cases need from outside: a place to store todos and a way to announce
//! changes. Infra implements both; test-utils has doubles.

use async_trait::async_trait;
use svc_util::prelude::*;

use super::{NewTodo, Todo, TodoEvent, TodoId};

/// Where todos are stored. Writes use optimistic concurrency: an update names the version it
/// expects, and only a todo at that version is replaced.
///
/// Errors are failures of the store itself; an adapter marks them as caused by a dependency
/// (`into_up()`) and sets `retry` when a retry may help.
#[async_trait]
pub trait TodoRepository: Send + Sync {
    /// Stores a new todo, not done, at version 1.
    ///
    /// # Errors
    ///
    /// When the store fails.
    async fn insert(&self, new: NewTodo) -> Result<Todo>;

    /// The todo with this id; `None` when there is none, which is not an error.
    ///
    /// # Errors
    ///
    /// When the store fails.
    async fn get(&self, id: TodoId) -> Result<Option<Todo>>;

    /// At most `limit` todos, in id order, which is creation order.
    ///
    /// # Errors
    ///
    /// When the store fails.
    async fn list(&self, limit: usize) -> Result<Vec<Todo>>;

    /// Replaces the stored todo with `todo` at the next version, only if the stored version
    /// equals `expected`.
    ///
    /// # Errors
    ///
    /// When the store fails.
    async fn update(&self, todo: &Todo, expected: u64) -> Result<UpdateOutcome>;

    /// Deletes the todo; whether there was one.
    ///
    /// # Errors
    ///
    /// When the store fails.
    async fn delete(&self, id: TodoId) -> Result<bool>;
}

/// What an update did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// Replaced; the stored todo, with its new version.
    Updated(Todo),
    /// Not replaced: the stored version is `current`.
    VersionMismatch {
        /// The version in the store.
        current: u64,
    },
    /// There is no todo with that id.
    NotFound,
}

/// Where todo events go. Delivery is at most once, after the change is stored; publishing
/// never fails and never waits.
pub trait TodoPublisher: Send + Sync {
    /// Publishes one event.
    fn publish(&self, event: TodoEvent);
}
