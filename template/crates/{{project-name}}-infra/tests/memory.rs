//! The todo store in memory keeps the port's contract, concurrent updates included.

use std::sync::Arc;

use svc_infra::memory::InMemoryTodoRepository;
use svc_test_utils::repo::todo_repository_contract;

#[tokio::test(flavor = "multi_thread")]
async fn the_store_keeps_the_contract() -> Result<(), String> {
    todo_repository_contract(Arc::new(InMemoryTodoRepository::default())).await
}
