//! Leaf crate: the error type of the whole workspace, the logging macros, and the small
//! helpers that configuration sections share.
//!
//! Other crates import the common items with `use svc_util::prelude::*;`.

pub mod de;
pub mod duration;
pub mod error;
mod log;
pub mod prelude;
