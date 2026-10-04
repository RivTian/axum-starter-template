//! The example feature: a todo list with optimistic concurrency.
//!
//! A [`Todo`] has a [`Title`] of 1 to 200 characters and a version that grows with every
//! change; a change names the version it expects, so concurrent changes cannot overwrite each
//! other. [`TodoUseCases`] holds the rules, over the ports [`TodoRepository`],
//! [`TodoPublisher`] and the clock, and announces every stored change as a [`TodoEvent`].

mod error;
mod events;
mod model;
mod ports;
mod usecases;

pub use error::{INVALID_TODO_TITLE, TODO_NOT_FOUND, TODO_VERSION_CONFLICT};
pub use events::TodoEvent;
pub use model::{NewTodo, Title, Todo, TodoId};
pub use ports::{TodoPublisher, TodoRepository, UpdateOutcome};
pub use usecases::TodoUseCases;
