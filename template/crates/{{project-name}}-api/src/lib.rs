//! The HTTP interface and its response contract: a success is the resource as JSON with a
//! status code that says what happened; a failure is an `application/problem+json` document,
//! rendered in one place from the error's class.
//!
//! [`server::HttpServer`] is the frontline service the binary hands to the supervisor;
//! [`router::router`] holds the routes and the middleware.

pub mod error;
mod extract;
mod middleware;
mod probes;
pub mod problem;
pub mod response;
pub mod router;
pub mod server;
pub mod settings;
pub mod state;
mod todo;
