//! How each error is answered: one row of the mapping table per class, by whether a
//! dependency caused it, and what the status code decides.

use axum::http::StatusCode;
use svc_api::problem::outcome;
use svc_util::prelude::*;
use tracing::Level;

fn kind(class: Class) -> ErrorType {
    // A leaked kind is fine in a test: each lives until the process ends.
    ErrorType::Kind(Box::leak(Box::new(ErrorKind::new("Sample", class))))
}

fn status(class: Class, upstream: bool) -> u16 {
    let error = Error::new(kind(class));
    let error = if upstream { error.into_up() } else { error };
    outcome(&error).status.as_u16()
}

#[test]
fn the_class_gives_the_status_and_a_dependency_turns_client_errors_into_server_errors() {
    let table = [
        (Class::InvalidInput, 422, 500),
        (Class::Unauthenticated, 401, 500),
        (Class::Forbidden, 403, 500),
        (Class::NotFound, 404, 500),
        (Class::Conflict, 409, 500),
        (Class::TooManyRequests, 429, 503),
        (Class::Unavailable, 503, 503),
        (Class::Timeout, 504, 504),
        (Class::Internal, 500, 500),
    ];
    for (class, own, upstream) in table {
        assert_eq!(
            (status(class, false), status(class, true)),
            (own, upstream),
            "{class}"
        );
    }
}

#[test]
fn an_http_status_error_answers_with_its_own_code() {
    let error = Error::new_down(ErrorType::HTTPStatus(405));
    assert_eq!(outcome(&error).status, StatusCode::METHOD_NOT_ALLOWED);
}

#[test]
fn the_status_decides_the_level_and_whether_the_detail_is_shown() {
    let cases = [
        (Class::InvalidInput, Level::DEBUG, true),
        (Class::Unauthenticated, Level::DEBUG, false),
        (Class::Forbidden, Level::DEBUG, false),
        (Class::TooManyRequests, Level::WARN, true),
        (Class::Unavailable, Level::WARN, false),
        (Class::Timeout, Level::WARN, false),
        (Class::Internal, Level::ERROR, false),
    ];
    for (class, level, expose) in cases {
        let answer = outcome(&Error::new(kind(class)));
        assert_eq!((answer.level, answer.expose), (level, expose), "{class}");
    }
}

#[test]
fn retry_after_needs_429_or_503_and_a_retryable_error() {
    for (class, retry, expected) in [
        (Class::TooManyRequests, true, true),
        (Class::Unavailable, true, true),
        (Class::Unavailable, false, false),
        (Class::Internal, true, false),
        (Class::Timeout, true, false),
    ] {
        let mut error = Error::new(kind(class));
        error.set_retry(retry);
        assert_eq!(
            outcome(&error).retry_after,
            expected,
            "{class} retry={retry}"
        );
    }
}
