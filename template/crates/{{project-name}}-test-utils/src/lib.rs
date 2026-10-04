//! Test doubles and shared test helpers. Every crate may use them as a dev-dependency; they
//! never reach the binary.
//!
//! The repository double comes with the contract every todo repository must keep, so the
//! real store and the double are checked against the same rules. Scripted services and
//! hand-sent signals drive the supervisor through any sequence of events, and captured logs
//! show what was logged.

pub mod clock;
pub mod logs;
pub mod publisher;
pub mod repo;
pub mod services;
pub mod signals;
