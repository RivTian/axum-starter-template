//! A todo repository in memory that can be told to misbehave, and the contract every todo
//! repository must keep. Infra runs the contract against its real repository and this double
//! alike, so the double cannot drift from the real one.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use svc_domain::prelude::*;
use svc_util::prelude::*;
use time::macros::datetime;

/// A repository in memory that can be told to misbehave: to fail, to be slow or to panic.
#[derive(Debug, Default)]
pub struct FakeTodoRepository {
    todos: Mutex<BTreeMap<TodoId, Todo>>,
    behaviour: Mutex<Behaviour>,
}

/// How every call of a [`FakeTodoRepository`] behaves before it does its work.
#[derive(Clone, Debug, Default)]
struct Behaviour {
    delay: Duration,
    panic: bool,
    failure: Option<ErrorType>,
}

impl FakeTodoRepository {
    /// From now on every call fails with an error of this type, caused by the dependency and
    /// not retryable, as an adapter would report a broken store.
    pub fn fail_with(&self, etype: ErrorType) {
        self.behaviour().failure = Some(etype);
    }

    /// From now on every call first waits this long.
    pub fn slow_down(&self, by: Duration) {
        self.behaviour().delay = by;
    }

    /// From now on every call panics.
    pub fn panic_on_call(&self) {
        self.behaviour().panic = true;
    }

    fn behaviour(&self) -> MutexGuard<'_, Behaviour> {
        // Every change sets one field: recover from a poisoned lock.
        self.behaviour
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn check(&self) -> Result<()> {
        let behaviour = self.behaviour().clone();
        tokio::time::sleep(behaviour.delay).await;
        if behaviour.panic {
            // Unwinds like a panic in a real store, which the HTTP layer must survive.
            std::panic::resume_unwind(Box::new("scripted panic in the repository"));
        }
        match behaviour.failure {
            Some(etype) => Err(Error::explain(etype, "injected failure").into_up()),
            None => Ok(()),
        }
    }

    // The map holds whole values that are replaced in one step, so a poisoned lock still
    // holds a consistent map: recover it.
    fn todos(&self) -> MutexGuard<'_, BTreeMap<TodoId, Todo>> {
        self.todos.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl TodoRepository for FakeTodoRepository {
    async fn insert(&self, new: NewTodo) -> Result<Todo> {
        self.check().await?;
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
        self.check().await?;
        Ok(self.todos().get(&id).cloned())
    }

    async fn list(&self, limit: usize) -> Result<Vec<Todo>> {
        self.check().await?;
        Ok(self.todos().values().take(limit).cloned().collect())
    }

    async fn update(&self, todo: &Todo, expected: u64) -> Result<UpdateOutcome> {
        self.check().await?;
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
        self.check().await?;
        Ok(self.todos().remove(&id).is_some())
    }
}

/// What broke the contract: the first rule broken, or an error the repository returned.
pub type ContractResult = std::result::Result<(), String>;

/// Turns an error the repository returned into the contract's failure.
trait OrBroken<T> {
    fn or_broken(self) -> std::result::Result<T, String>;
}

impl<T> OrBroken<T> for Result<T> {
    fn or_broken(self) -> std::result::Result<T, String> {
        self.map_err(|error| format!("the repository failed: {}", error.fields().chain))
    }
}

fn check(holds: bool, rule: &str) -> ContractResult {
    if holds {
        Ok(())
    } else {
        Err(format!("broken rule: {rule}"))
    }
}

/// Checks that an empty repository keeps the port's contract: insert at version 1, get and
/// its `None`, list in id order within the limit, update only at the expected version, and
/// delete. Then checks that concurrent updates at one version replace the todo only once.
///
/// # Errors
///
/// The first rule the repository breaks, or an error it returned.
pub async fn todo_repository_contract(repository: Arc<dyn TodoRepository>) -> ContractResult {
    let title = |text: &str| Title::new(text).or_broken();
    let first = TodoId::at(datetime!(2026-01-01 00:00:00 UTC));
    let second = TodoId::at(datetime!(2026-01-01 00:00:01 UTC));
    let third = TodoId::at(datetime!(2026-01-01 00:00:02 UTC));

    // Inserted out of order, so that listing must sort.
    for (id, text) in [(second, "second"), (first, "first"), (third, "third")] {
        let new = NewTodo {
            id,
            title: title(text)?,
        };
        let todo = repository.insert(new).await.or_broken()?;
        check(
            todo.version == 1 && !todo.done && todo.id == id,
            "insert stores a new todo at version 1, not done",
        )?;
    }
    let stored = repository.get(first).await.or_broken()?;
    check(
        stored.as_ref().map(|todo| todo.title.as_str()) == Some("first"),
        "get returns the stored todo",
    )?;
    let unknown = TodoId::at(datetime!(2026-01-02 00:00 UTC));
    check(
        repository.get(unknown).await.or_broken()?.is_none(),
        "get of an unknown id is None",
    )?;

    let listed: Vec<TodoId> = (repository.list(2).await.or_broken()?.iter())
        .map(|todo| todo.id)
        .collect();
    check(
        listed == [first, second],
        "list returns todos in id order, at most the limit",
    )?;

    let mut done = stored.ok_or("get returned nothing")?;
    done.done = true;
    let updated = repository.update(&done, 1).await.or_broken()?;
    check(
        matches!(&updated, UpdateOutcome::Updated(todo) if todo.version == 2 && todo.done),
        "update at the expected version stores the change at the next version",
    )?;
    let stale = repository.update(&done, 1).await.or_broken()?;
    check(
        stale == UpdateOutcome::VersionMismatch { current: 2 },
        "update at a stale version reports the current version",
    )?;
    let missing = Todo {
        id: unknown,
        ..done
    };
    check(
        repository.update(&missing, 1).await.or_broken()? == UpdateOutcome::NotFound,
        "update of an unknown id reports NotFound",
    )?;

    check(
        repository.delete(first).await.or_broken()?,
        "delete of a stored todo is true",
    )?;
    check(
        !repository.delete(first).await.or_broken()?,
        "delete of a deleted todo is false",
    )?;
    check(
        repository.get(first).await.or_broken()?.is_none(),
        "a deleted todo is gone",
    )?;
    concurrent_updates(repository).await
}

/// Sixteen tasks update one todo at version 1 at the same time: exactly one wins, and every
/// other one is told the current version.
async fn concurrent_updates(repository: Arc<dyn TodoRepository>) -> ContractResult {
    let id = TodoId::at(datetime!(2026-01-03 00:00 UTC));
    let new = NewTodo {
        id,
        title: Title::new("raced").or_broken()?,
    };
    let todo = repository.insert(new).await.or_broken()?;
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let (repository, done) = (
                repository.clone(),
                Todo {
                    done: true,
                    ..todo.clone()
                },
            );
            tokio::spawn(async move { repository.update(&done, 1).await })
        })
        .collect();
    let mut won = 0;
    for task in tasks {
        match task.await.map_err(|e| e.to_string())?.or_broken()? {
            UpdateOutcome::Updated(_) => won += 1,
            UpdateOutcome::VersionMismatch { current: 2 } => {}
            other => return Err(format!("broken rule: a raced update returned {other:?}")),
        }
    }
    check(
        won == 1,
        "concurrent updates at one version replace the todo once",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use svc_domain::prelude::*;
    use svc_util::prelude::*;

    use super::{FakeTodoRepository, todo_repository_contract};

    #[tokio::test(flavor = "multi_thread")]
    async fn the_fake_keeps_the_contract() -> Result<(), String> {
        todo_repository_contract(Arc::new(FakeTodoRepository::default())).await
    }

    #[tokio::test]
    async fn an_injected_failure_breaks_every_call() -> Result<(), String> {
        let repository = FakeTodoRepository::default();
        repository.fail_with(ErrorType::ConnectRefused);
        let Err(error) = repository.list(10).await else {
            return Err("the call succeeded".to_string());
        };
        let fields = error.fields();
        assert_eq!(
            (fields.etype, fields.source, fields.retry),
            ("ConnectRefused", "upstream", false)
        );
        Ok(())
    }
}
