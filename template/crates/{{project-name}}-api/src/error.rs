//! The error kinds of the HTTP interface.

use svc_util::prelude::*;

/// A request body that is valid JSON but does not have the expected shape.
pub const REQUEST_REJECTED: ErrorType = ErrorType::Kind(
    &ErrorKind::new("RequestRejected", Class::InvalidInput).titled("Request rejected"),
);
