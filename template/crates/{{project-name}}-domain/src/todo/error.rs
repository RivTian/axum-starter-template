//! The error kinds of the todo feature.

use svc_util::prelude::*;

/// No todo has the given id.
pub const TODO_NOT_FOUND: ErrorType =
    ErrorType::Kind(&ErrorKind::new("TodoNotFound", Class::NotFound).titled("Todo not found"));

/// The todo changed since the caller read it: the versions differ.
pub const TODO_VERSION_CONFLICT: ErrorType = ErrorType::Kind(
    &ErrorKind::new("TodoVersionConflict", Class::Conflict).titled("Todo version conflict"),
);

/// A title is empty or longer than 200 characters.
pub const INVALID_TODO_TITLE: ErrorType = ErrorType::Kind(
    &ErrorKind::new("InvalidTodoTitle", Class::InvalidInput).titled("Invalid todo title"),
);
