//! Adapters for the domain's ports: the todo store in memory, the system clock, and the todo
//! events on the runtime's event bus with the background service that logs them.
//!
//! A real store, such as a database, goes next to `memory` and implements the same port; the
//! contract in test-utils checks it the same way.

pub mod clock;
pub mod event_log;
pub mod memory;
pub mod publisher;
