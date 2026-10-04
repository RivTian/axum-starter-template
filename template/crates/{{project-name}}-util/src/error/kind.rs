//! Error kinds and their classes.

use std::fmt;

use super::ErrorType;

/// The nature of a failure, independent of any transport. The HTTP layer derives the status
/// code from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// The caller's data breaks a rule.
    InvalidInput,
    /// The caller is not identified.
    Unauthenticated,
    /// The caller is identified but not allowed to do this.
    Forbidden,
    /// The target does not exist.
    NotFound,
    /// The request conflicts with the current state, such as a stale version.
    Conflict,
    /// The caller exceeded a quota.
    TooManyRequests,
    /// A dependency is unavailable or failed to answer properly.
    Unavailable,
    /// Waiting took too long.
    Timeout,
    /// A defect in this service or a broken invariant.
    Internal,
}

impl Class {
    /// The name, as logged in `error.class`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Class::InvalidInput => "InvalidInput",
            Class::Unauthenticated => "Unauthenticated",
            Class::Forbidden => "Forbidden",
            Class::NotFound => "NotFound",
            Class::Conflict => "Conflict",
            Class::TooManyRequests => "TooManyRequests",
            Class::Unavailable => "Unavailable",
            Class::Timeout => "Timeout",
            Class::Internal => "Internal",
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error kind this project defines: a `PascalCase` name for logs and problem types, a
/// class, and a short title shown to callers of client errors.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ErrorKind {
    name: &'static str,
    class: Class,
    title: &'static str,
}

impl ErrorKind {
    /// A kind with this name, class and title.
    #[must_use]
    pub const fn new(name: &'static str, class: Class, title: &'static str) -> Self {
        ErrorKind { name, class, title }
    }

    /// The name, such as `TodoNotFound`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The class.
    #[must_use]
    pub const fn class(&self) -> Class {
        self.class
    }

    /// The title, such as `Todo not found`.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        self.title
    }
}

impl ErrorType {
    /// The class of any error type: a kind carries its own, and the upstream types are
    /// classified here once. Everything not listed, including the upstream variants this
    /// project does not use, is [`Class::Internal`].
    #[must_use]
    pub const fn class(&self) -> Class {
        match self {
            ErrorType::Kind(kind) => kind.class,
            ErrorType::HTTPStatus(code) => match *code {
                401 => Class::Unauthenticated,
                403 => Class::Forbidden,
                404 => Class::NotFound,
                408 | 504 => Class::Timeout,
                409 => Class::Conflict,
                429 => Class::TooManyRequests,
                503 => Class::Unavailable,
                400..=499 => Class::InvalidInput,
                _ => Class::Internal,
            },
            ErrorType::ConnectTimedout
            | ErrorType::TLSHandshakeTimedout
            | ErrorType::ReadTimedout
            | ErrorType::WriteTimedout => Class::Timeout,
            ErrorType::ConnectRefused
            | ErrorType::ConnectNoRoute
            | ErrorType::ConnectError
            | ErrorType::ConnectProxyFailure
            | ErrorType::TLSWantX509Lookup
            | ErrorType::TLSHandshakeFailure
            | ErrorType::InvalidCert
            | ErrorType::HandshakeError
            | ErrorType::InvalidHTTPHeader
            | ErrorType::H1Error
            | ErrorType::H2Error
            | ErrorType::H2Downgrade
            | ErrorType::InvalidH2
            | ErrorType::ReadError
            | ErrorType::WriteError
            | ErrorType::ConnectionClosed => Class::Unavailable,
            _ => Class::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Class, ErrorKind};
    use crate::error::ErrorType;

    const MISSING: ErrorType = ErrorType::Kind(&ErrorKind::new(
        "ThingMissing",
        Class::NotFound,
        "Thing missing",
    ));

    #[test]
    fn a_kind_carries_its_name_class_and_title() {
        assert_eq!(MISSING.as_str(), "ThingMissing");
        assert_eq!(MISSING.class(), Class::NotFound);
        let ErrorType::Kind(kind) = MISSING else {
            return;
        };
        assert_eq!(kind.title(), "Thing missing");
    }

    #[test]
    fn upstream_types_and_status_codes_have_a_class() {
        assert_eq!(ErrorType::ReadTimedout.class(), Class::Timeout);
        assert_eq!(ErrorType::ConnectRefused.class(), Class::Unavailable);
        assert_eq!(ErrorType::BindError.class(), Class::Internal);
        assert_eq!(ErrorType::HTTPStatus(404).class(), Class::NotFound);
        assert_eq!(ErrorType::HTTPStatus(413).class(), Class::InvalidInput);
        assert_eq!(ErrorType::HTTPStatus(500).class(), Class::Internal);
    }
}
