//! The success side of the HTTP contract: the resource itself as JSON, with a status code that
//! says what happened. Failures are problem documents, in [`crate::problem`].

use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use svc_util::prelude::*;

use crate::problem::pending;

/// What a handler returns when it succeeds.
#[derive(Debug)]
pub enum ApiResponse<T: Serialize> {
    /// 200 with the body.
    Ok(T),
    /// 201 with a `Location` header and the body.
    Created {
        /// The path of the new resource.
        location: String,
        /// The new resource.
        body: T,
    },
    /// 202 with the body.
    Accepted(T),
    /// 204 without a body.
    NoContent,
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        let (status, location, body) = match self {
            ApiResponse::Ok(body) => (StatusCode::OK, None, body),
            ApiResponse::Created { location, body } => (StatusCode::CREATED, Some(location), body),
            ApiResponse::Accepted(body) => (StatusCode::ACCEPTED, None, body),
            ApiResponse::NoContent => return StatusCode::NO_CONTENT.into_response(),
        };
        // A body that cannot be serialized, or a location that is not a header value, is this
        // service's failure: it becomes a problem like any other error.
        let bytes = match serde_json::to_vec(&body) {
            Ok(bytes) => bytes,
            Err(error) => {
                let context = "the response body could not be serialized";
                return pending(Error::because(ErrorType::InternalError, context, error));
            }
        };
        let json = HeaderValue::from_static("application/json");
        let mut response = (status, [(CONTENT_TYPE, json)], bytes).into_response();
        if let Some(location) = location {
            match HeaderValue::try_from(location) {
                Ok(value) => {
                    response.headers_mut().insert(LOCATION, value);
                }
                Err(error) => {
                    let context = "the Location header is not a valid header value";
                    return pending(Error::because(ErrorType::InternalError, context, error));
                }
            }
        }
        response
    }
}
