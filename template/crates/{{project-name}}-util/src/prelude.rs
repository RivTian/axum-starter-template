//! Items that other crates import with `use svc_util::prelude::*;`.

pub use crate::error::{
    BError, Class, Context, Error, ErrorKind, ErrorSource, ErrorType, OkOrErr, OrErr, Result,
};
pub use crate::{log_at_level, log_error};
