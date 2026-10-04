//! The process runtime: what a service is, the supervisor that starts, watches and stops the
//! services, the signals it reacts to, readiness, and the in-process event bus.
//!
//! Runtime knows no business and no transport. Services such as the HTTP server and event
//! subscribers live in the crates above and implement [`service::Service`]; the binary hands
//! them to a [`supervisor::Supervisor`].

pub mod bus;
pub mod error;
pub mod health;
pub mod phase;
pub mod prelude;
pub mod service;
pub mod settings;
pub mod signal;
pub mod supervisor;
