//! The todo endpoints and their JSON shapes. The rules live in the domain; a handler only
//! translates between JSON and the use cases.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use svc_domain::todo::{Todo, TodoId};
use svc_util::prelude::*;

use crate::extract::{ApiJson, ApiPath};
use crate::problem::{ApiError, ApiResult};
use crate::response::ApiResponse;
use crate::state::AppState;

/// The todos listed when the request has no `limit`.
const DEFAULT_LIMIT: usize = 50;
/// The most todos one request lists.
const MAX_LIMIT: usize = 500;

/// A todo as the API shows it.
#[derive(Debug, Serialize)]
pub(crate) struct TodoDto {
    id: String,
    title: String,
    done: bool,
    version: u64,
}

impl From<Todo> for TodoDto {
    fn from(todo: Todo) -> Self {
        TodoDto {
            id: todo.id.to_string(),
            title: todo.title.as_str().to_string(),
            done: todo.done,
            version: todo.version,
        }
    }
}

/// The body of `POST /v1/todos`.
#[derive(Debug, Deserialize)]
pub(crate) struct CreateTodo {
    title: String,
}

/// The body of `POST /v1/todos/{id}/complete`.
#[derive(Debug, Deserialize)]
pub(crate) struct CompleteTodo {
    version: u64,
}

/// The query of `GET /v1/todos`.
#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    limit: Option<String>,
}

/// `POST /v1/todos`: 201 with the new todo and its `Location`.
pub(crate) async fn create(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CreateTodo>,
) -> ApiResult<TodoDto> {
    let todo = state.todos.create(&body.title).await?;
    Ok(ApiResponse::Created {
        location: format!("/v1/todos/{}", todo.id),
        body: todo.into(),
    })
}

/// `GET /v1/todos/{id}`.
pub(crate) async fn get(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<TodoId>,
) -> ApiResult<TodoDto> {
    Ok(ApiResponse::Ok(state.todos.get(id).await?.into()))
}

/// `GET /v1/todos?limit=N`: the oldest todos first, 50 unless `limit` says otherwise.
pub(crate) async fn list(
    State(state): State<AppState>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> ApiResult<Vec<TodoDto>> {
    let limit = query
        .ok()
        .and_then(|Query(query)| match query.limit {
            None => Some(DEFAULT_LIMIT),
            Some(text) => text
                .parse()
                .ok()
                .filter(|limit| (1..=MAX_LIMIT).contains(limit)),
        })
        .ok_or_else(|| {
            ApiError(
                Error::explain(
                    ErrorType::HTTPStatus(400),
                    "limit must be an integer from 1 to 500",
                )
                .into_down(),
            )
        })?;
    let todos = state.todos.list(limit).await?;
    Ok(ApiResponse::Ok(
        todos.into_iter().map(TodoDto::from).collect(),
    ))
}

/// `POST /v1/todos/{id}/complete`: 409 when `version` is not the current one.
pub(crate) async fn complete(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<TodoId>,
    ApiJson(body): ApiJson<CompleteTodo>,
) -> ApiResult<TodoDto> {
    Ok(ApiResponse::Ok(
        state.todos.complete(id, body.version).await?.into(),
    ))
}

/// `DELETE /v1/todos/{id}`: 204.
pub(crate) async fn delete(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<TodoId>,
) -> ApiResult<()> {
    state.todos.delete(id).await?;
    Ok(ApiResponse::NoContent)
}
