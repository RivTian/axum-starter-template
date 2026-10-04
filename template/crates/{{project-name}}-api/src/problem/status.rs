//! How an error is answered: the status code from the error's class, and everything else from
//! the status code.

use axum::http::StatusCode;
use svc_util::prelude::*;
use tracing::Level;

/// How to answer an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The status code.
    pub status: StatusCode,
    /// The level the error is logged at.
    pub level: Level,
    /// Whether the problem shows the error's context as `detail`.
    pub expose: bool,
    /// Whether the response carries `Retry-After`.
    pub retry_after: bool,
}

/// The outcome of an error. `HTTPStatus` errors answer with their own code; every other error
/// with the code of its class, where a client error caused by a dependency is this service's
/// failure.
#[must_use]
pub fn outcome(error: &Error) -> Outcome {
    let code = match error.etype() {
        ErrorType::HTTPStatus(code) => *code,
        etype => status_of(etype.class(), error.esource()),
    };
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let code = status.as_u16();
    Outcome {
        status,
        level: match code {
            429 | 503 | 504 => Level::WARN,
            500.. => Level::ERROR,
            _ => Level::DEBUG,
        },
        expose: status.is_client_error() && !matches!(code, 401 | 403),
        retry_after: matches!(code, 429 | 503) && error.retry(),
    }
}

fn status_of(class: Class, source: &ErrorSource) -> u16 {
    let code = match class {
        Class::InvalidInput => 422,
        Class::Unauthenticated => 401,
        Class::Forbidden => 403,
        Class::NotFound => 404,
        Class::Conflict => 409,
        Class::TooManyRequests => 429,
        Class::Unavailable => 503,
        Class::Timeout => 504,
        Class::Internal => 500,
    };
    match (source, code) {
        (ErrorSource::Upstream, 429) => 503,
        (ErrorSource::Upstream, 400..=499) => 500,
        _ => code,
    }
}
