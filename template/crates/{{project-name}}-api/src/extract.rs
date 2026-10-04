//! Extractors whose rejections join the HTTP contract: each becomes an error the problem
//! middleware renders, instead of the framework's plain-text answer.

use std::fmt;
use std::str::FromStr;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;
use svc_util::prelude::*;

use crate::error::REQUEST_REJECTED;
use crate::problem::ApiError;

/// A JSON request body. Too large gives 413, a missing JSON content type 415, broken JSON
/// 400, and JSON of the wrong shape 422 [`REQUEST_REJECTED`]; the reason is the detail.
#[derive(Debug)]
pub(crate) struct ApiJson<T>(pub(crate) T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_rejection(&rejection)),
        }
    }
}

fn json_rejection(rejection: &JsonRejection) -> ApiError {
    let reason = rejection.body_text();
    let etype = match rejection.status() {
        StatusCode::UNPROCESSABLE_ENTITY => REQUEST_REJECTED,
        status => ErrorType::HTTPStatus(status.as_u16()),
    };
    ApiError(Error::explain(etype, reason).into_down())
}

/// The path's one parameter, parsed with `FromStr`; one that does not parse gives 400 with
/// the reason as the detail.
#[derive(Debug)]
pub(crate) struct ApiPath<T>(pub(crate) T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: FromStr + Send,
    T::Err: fmt::Display,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(text) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| bad_path(rejection.body_text()))?;
        text.parse()
            .map(ApiPath)
            .map_err(|error| bad_path(format!("invalid path parameter {text:?}: {error}")))
    }
}

fn bad_path(reason: String) -> ApiError {
    ApiError(Error::explain(ErrorType::HTTPStatus(400), reason).into_down())
}
