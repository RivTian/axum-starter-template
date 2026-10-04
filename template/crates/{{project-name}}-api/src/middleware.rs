//! The middleware of every request, from the outside in: the request id, the request span
//! with the access log, problem rendering with the request timeout (in [`crate::problem`]),
//! the panic catcher and the body limit. The last two are inside problem rendering, so their
//! answers become problem documents too.

mod access;
mod request_id;

use std::any::Any;

use axum::extract::DefaultBodyLimit;
use axum::response::Response;
use svc_util::prelude::*;
use tower_http::catch_panic::CatchPanicLayer;

pub(crate) use access::access_log;
pub(crate) use request_id::{RequestId, request_id};

use crate::problem::pending;

/// A body larger than `server.body_limit_bytes` makes the JSON extractor answer 413.
pub(crate) fn body_limit(bytes: usize) -> DefaultBodyLimit {
    DefaultBodyLimit::max(bytes)
}

type PanicHandler = fn(Box<dyn Any + Send + 'static>) -> Response;

/// A handler that panics gets a 500 problem, logged with the request id; the panic hook logs
/// the panic itself.
pub(crate) fn catch_panic() -> CatchPanicLayer<PanicHandler> {
    CatchPanicLayer::custom(on_panic as PanicHandler)
}

fn on_panic(_payload: Box<dyn Any + Send + 'static>) -> Response {
    pending(Error::explain(ErrorType::HTTPStatus(500), "the handler panicked").into_in())
}
