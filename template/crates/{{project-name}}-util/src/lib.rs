//! Leaf crate: the error type of the whole workspace and the logging macros.
//!
//! Other crates import the common items with `use svc_util::prelude::*;`.

pub mod error;
mod log;
pub mod prelude;
