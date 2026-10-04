//! The error type of the whole workspace.
//!
//! [`Error`] is the error type of pingora-error, vendored unchanged apart from one added
//! variant, [`ErrorType::Kind`]: an error kind this project defines, carrying its own
//! [`Class`]. A crate defines each of its kinds once, as a constant in its `error.rs`:
//!
//! ```
//! use svc_util::error::{Class, Error, ErrorKind, ErrorType};
//!
//! /// No todo has the given id.
//! pub const TODO_NOT_FOUND: ErrorType =
//!     ErrorType::Kind(&ErrorKind::new("TodoNotFound", Class::NotFound).titled("Todo not found"));
//!
//! let error = Error::explain(TODO_NOT_FOUND, "todo 7 does not exist");
//! assert_eq!(error.etype().class(), Class::NotFound);
//! ```
//!
//! The class travels with the error, so the HTTP layer maps it to a status code with a plain
//! `match`, and nothing has to register the kinds anywhere.

mod fields;
mod kind;
mod pingora;

pub use fields::{Chain, Fields};
pub use kind::{Class, ErrorKind};
pub use pingora::{
    BError, Context, Error, ErrorSource, ErrorTrait, ErrorType, ImmutStr, OkOrErr, OrErr, Result,
    RetryType,
};
