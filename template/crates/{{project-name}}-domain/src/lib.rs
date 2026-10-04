//! The business model, free of I/O: entities and their rules, the use cases, the ports the
//! use cases need from outside, and the events they announce.
//!
//! Each feature is a module of its own; the example feature is [`todo`](mod@todo). Ports
//! shared by every feature, such as the [`clock`], sit next to the features. Infra implements
//! the ports; the domain never sees an HTTP status code.

pub mod clock;
pub mod prelude;
pub mod todo;
