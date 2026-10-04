//! The cause chain of an error, and the fields of a log event about it.

use std::error::Error as StdError;

use super::{BError, Error, ErrorSource};

/// The cause chain of an error, starting with the error itself; see [`Error::chain`].
///
/// Hops of this project's type downcast to [`Error`]; foreign errors, such as
/// `std::io::Error`, do not.
#[derive(Clone, Debug)]
pub struct Chain<'a> {
    next: Option<&'a (dyn StdError + 'static)>,
}

impl<'a> Iterator for Chain<'a> {
    type Item = &'a (dyn StdError + 'static);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        self.next = match current.downcast_ref::<Error>() {
            Some(typed) => typed.cause.as_deref().map(|cause| unbox(cause)),
            None => current.source().map(unbox),
        };
        Some(current)
    }
}

/// A cause of this project's type is stored as a `BError`; the chain yields the `Error`
/// inside, so that every hop of this type downcasts to [`Error`].
fn unbox<'a>(err: &'a (dyn StdError + 'static)) -> &'a (dyn StdError + 'static) {
    match err.downcast_ref::<BError>() {
        Some(boxed) => boxed.as_ref(),
        None => err,
    }
}

/// The fields of a log event about an error; `log_error!` writes them under the names given
/// below. They are taken from the error's members, never from its display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    /// `error.type`: the name of the error type, such as `TodoNotFound`.
    pub etype: &'static str,
    /// `error.class`: the class, such as `NotFound`.
    pub class: &'static str,
    /// `error.source`: `upstream`, `downstream`, `internal` or `unset`.
    pub source: &'static str,
    /// `error.retry`: whether retrying may help.
    pub retry: bool,
    /// `error.context`: the context, empty when there is none.
    pub context: String,
    /// `error.chain`: the type names along the cause chain, comma separated; foreign errors
    /// appear as `external`.
    pub chain: String,
}

impl Error {
    /// The cause chain, starting with this error. Errors of this project's type are followed
    /// through their `cause`, foreign errors through their `source()`.
    #[must_use]
    pub fn chain(&self) -> Chain<'_> {
        Chain { next: Some(self) }
    }

    /// The fields a log event about this error carries.
    #[must_use]
    pub fn fields(&self) -> Fields {
        let source = match self.esource {
            ErrorSource::Upstream => "upstream",
            ErrorSource::Downstream => "downstream",
            ErrorSource::Internal => "internal",
            ErrorSource::Unset => "unset",
        };
        let chain: Vec<&str> = self
            .chain()
            .map(|hop| {
                hop.downcast_ref::<Error>()
                    .map_or("external", |typed| typed.etype.as_str())
            })
            .collect();
        Fields {
            etype: self.etype.as_str(),
            class: self.etype.class().as_str(),
            source,
            retry: self.retry(),
            context: self
                .context
                .as_ref()
                .map(|context| context.as_str().to_string())
                .unwrap_or_default(),
            chain: chain.join(","),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::io;

    use crate::error::{Class, Error, ErrorKind, ErrorType};

    const TODO_MISSING: ErrorType = ErrorType::Kind(&ErrorKind::new(
        "TodoMissing",
        Class::NotFound,
        "Todo missing",
    ));
    const STORE_BROKEN: ErrorType = ErrorType::Kind(&ErrorKind::new(
        "StoreBroken",
        Class::Unavailable,
        "Store broken",
    ));

    fn names(error: &Error) -> Vec<String> {
        error
            .chain()
            .map(|hop| match hop.downcast_ref::<Error>() {
                Some(typed) => typed.etype.as_str().to_string(),
                None => format!("foreign: {hop}"),
            })
            .collect()
    }

    #[derive(Debug)]
    struct Outer(io::Error);

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer failure")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn the_chain_follows_typed_and_foreign_causes() {
        let foreign = Outer(io::Error::other("disk full"));
        let middle = Error::because(STORE_BROKEN, "write failed", foreign);
        let outer = Error::because(TODO_MISSING, "while saving", middle);
        assert_eq!(
            names(&outer),
            [
                "TodoMissing",
                "StoreBroken",
                "foreign: outer failure",
                "foreign: disk full"
            ]
        );
    }

    #[test]
    fn fields_come_from_the_members_not_the_display_text() {
        let inner = Error::explain(STORE_BROKEN, "store is gone").into_up();
        let mut outer = Error::because(TODO_MISSING, "while loading", inner);
        outer.set_retry(true);
        let fields = outer.fields();
        assert_eq!(
            (fields.etype, fields.class, fields.source, fields.retry),
            ("TodoMissing", "NotFound", "unset", true)
        );
        assert_eq!(fields.context, "while loading");
        assert_eq!(fields.chain, "TodoMissing,StoreBroken");
    }
}
