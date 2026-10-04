//! Items that other crates import with `use svc_domain::prelude::*;`: the ports and the types
//! that cross them, which adapters and test doubles need.

pub use crate::clock::Clock;
pub use crate::todo::{
    NewTodo, Title, Todo, TodoEvent, TodoId, TodoPublisher, TodoRepository, UpdateOutcome,
};
