//! Stores in memory, one per feature. They lose everything when the process stops, which
//! suits the example and tests; replace them with a real store behind the same ports.

mod todo;

pub use todo::InMemoryTodoRepository;
