//! The todo use cases, with the repository, clock and publisher doubles from test-utils.

use std::error::Error as StdError;
use std::sync::Arc;

use svc_domain::todo::{Todo, TodoEvent, TodoId, TodoUseCases};
use svc_test_utils::clock::FakeClock;
use svc_test_utils::publisher::RecordingPublisher;
use svc_test_utils::repo::FakeTodoRepository;
use svc_util::error::{ErrorSource, ErrorType, Result};

type TestResult = std::result::Result<(), Box<dyn StdError>>;

struct Setup {
    service: TodoUseCases,
    repository: Arc<FakeTodoRepository>,
    events: Arc<RecordingPublisher>,
}

fn setup() -> Setup {
    let repository = Arc::new(FakeTodoRepository::default());
    let events = Arc::new(RecordingPublisher::default());
    let service = TodoUseCases::new(
        repository.clone(),
        events.clone(),
        Arc::new(FakeClock::default()),
    );
    Setup {
        service,
        repository,
        events,
    }
}

/// The error type and context of a failed call.
fn failure<T: std::fmt::Debug>(
    result: Result<T>,
) -> std::result::Result<(String, String), Box<dyn StdError>> {
    let Err(error) = result else {
        return Err(format!("expected an error, got {result:?}").into());
    };
    let fields = error.fields();
    Ok((fields.etype.to_string(), fields.context))
}

fn pair(etype: &str, context: &str) -> (String, String) {
    (etype.to_string(), context.to_string())
}

#[tokio::test]
async fn create_stores_a_new_todo_and_announces_it() -> TestResult {
    let s = setup();
    let todo = s.service.create("Buy milk").await?;
    assert_eq!(
        (todo.title.as_str(), todo.done, todo.version),
        ("Buy milk", false, 1)
    );
    assert_eq!(s.service.get(todo.id).await?, todo);
    assert_eq!(s.events.events(), [TodoEvent::Created { id: todo.id }]);
    Ok(())
}

#[tokio::test]
async fn a_bad_title_stores_nothing() -> TestResult {
    let s = setup();
    assert_eq!(
        failure(s.service.create("").await)?,
        pair("InvalidTodoTitle", "todo title must not be empty")
    );
    assert_eq!(
        failure(s.service.create(&"x".repeat(201)).await)?,
        pair(
            "InvalidTodoTitle",
            "todo title must have at most 200 characters"
        )
    );
    assert_eq!(s.service.list(10).await?, []);
    assert_eq!(s.events.events(), []);
    Ok(())
}

#[tokio::test]
async fn an_unknown_todo_is_not_found() -> TestResult {
    let s = setup();
    let id: TodoId = "0199a8f0-0000-7000-8000-000000000000".parse()?;
    let missing = pair("TodoNotFound", &format!("todo {id} does not exist"));
    assert_eq!(failure(s.service.get(id).await)?, missing);
    assert_eq!(failure(s.service.complete(id, 1).await)?, missing);
    assert_eq!(failure(s.service.delete(id).await)?, missing);
    assert_eq!(s.events.events(), []);
    Ok(())
}

#[tokio::test]
async fn list_returns_the_oldest_first_within_the_limit() -> TestResult {
    let s = setup();
    let mut created = Vec::new();
    for title in ["one", "two", "three"] {
        created.push(s.service.create(title).await?.id);
    }
    let listed: Vec<TodoId> = s
        .service
        .list(2)
        .await?
        .iter()
        .map(|todo| todo.id)
        .collect();
    assert_eq!(listed, created[..2]);
    assert_eq!(s.service.list(10).await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn complete_needs_the_current_version() -> TestResult {
    let s = setup();
    let todo = s.service.create("Buy milk").await?;
    let done = s.service.complete(todo.id, 1).await?;
    assert_eq!((done.done, done.version), (true, 2));
    assert_eq!(
        failure(s.service.complete(todo.id, 1).await)?,
        pair("TodoVersionConflict", "expected version 1, found 2")
    );
    // Completing again at the current version is a change like any other.
    assert_eq!(s.service.complete(todo.id, 2).await?.version, 3);
    assert_eq!(
        s.events.events(),
        [
            TodoEvent::Created { id: todo.id },
            TodoEvent::Completed { id: todo.id },
            TodoEvent::Completed { id: todo.id }
        ]
    );
    Ok(())
}

#[tokio::test]
async fn of_two_completions_at_one_version_exactly_one_wins() -> TestResult {
    let s = setup();
    let todo = s.service.create("Buy milk").await?;
    let (a, b) = tokio::join!(
        s.service.complete(todo.id, 1),
        s.service.complete(todo.id, 1)
    );
    let won: Vec<&Todo> = [&a, &b]
        .into_iter()
        .filter_map(|r| r.as_ref().ok())
        .collect();
    assert_eq!(won.len(), 1);
    let lost = if a.is_ok() { b } else { a };
    assert_eq!(failure(lost)?.0, "TodoVersionConflict");
    let completed = s
        .events
        .events()
        .into_iter()
        .filter(|event| matches!(event, TodoEvent::Completed { .. }))
        .count();
    assert_eq!(completed, 1);
    Ok(())
}

#[tokio::test]
async fn delete_removes_the_todo_and_announces_it() -> TestResult {
    let s = setup();
    let todo = s.service.create("Buy milk").await?;
    s.service.delete(todo.id).await?;
    assert_eq!(failure(s.service.get(todo.id).await)?.0, "TodoNotFound");
    assert_eq!(failure(s.service.delete(todo.id).await)?.0, "TodoNotFound");
    assert_eq!(
        s.events.events(),
        [
            TodoEvent::Created { id: todo.id },
            TodoEvent::Deleted { id: todo.id }
        ]
    );
    Ok(())
}

#[tokio::test]
async fn repository_failures_pass_through_unchanged() -> TestResult {
    let s = setup();
    s.repository.fail_with(ErrorType::ConnectRefused);
    let Err(error) = s.service.create("Buy milk").await else {
        return Err("the call succeeded".into());
    };
    assert_eq!(error.etype(), &ErrorType::ConnectRefused);
    assert_eq!(error.esource(), &ErrorSource::Upstream);
    assert_eq!(s.events.events(), []);
    Ok(())
}
