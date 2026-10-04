//! The todo store in memory.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use svc_domain::prelude::*;
use svc_util::prelude::*;

/// The todo store in memory, in id order, which is creation order. One lock guards the map
/// and is never held across an `.await`.
#[derive(Debug, Default)]
pub struct InMemoryTodoRepository {
    todos: Mutex<BTreeMap<TodoId, Todo>>,
}

impl InMemoryTodoRepository {
    fn todos(&self) -> MutexGuard<'_, BTreeMap<TodoId, Todo>> {
        // Every write replaces whole values in one step, so a lock poisoned by a panic still
        // guards a consistent map: recover it.
        self.todos.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl TodoRepository for InMemoryTodoRepository {
    async fn insert(&self, new: NewTodo) -> Result<Todo> {
        let todo = Todo {
            id: new.id,
            title: new.title,
            done: false,
            version: 1,
        };
        self.todos().insert(todo.id, todo.clone());
        Ok(todo)
    }

    async fn get(&self, id: TodoId) -> Result<Option<Todo>> {
        Ok(self.todos().get(&id).cloned())
    }

    async fn list(&self, limit: usize) -> Result<Vec<Todo>> {
        Ok(self.todos().values().take(limit).cloned().collect())
    }

    async fn update(&self, todo: &Todo, expected: u64) -> Result<UpdateOutcome> {
        let mut todos = self.todos();
        let Some(stored) = todos.get_mut(&todo.id) else {
            return Ok(UpdateOutcome::NotFound);
        };
        if stored.version != expected {
            return Ok(UpdateOutcome::VersionMismatch {
                current: stored.version,
            });
        }
        *stored = Todo {
            version: expected + 1,
            ..todo.clone()
        };
        Ok(UpdateOutcome::Updated(stored.clone()))
    }

    async fn delete(&self, id: TodoId) -> Result<bool> {
        Ok(self.todos().remove(&id).is_some())
    }
}
