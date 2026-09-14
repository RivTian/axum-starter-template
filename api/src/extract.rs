//! The only prelaid module: adding the first DTO must not silently bypass the
//! existing JSON error envelope. Do not re-export axum's raw extractors elsewhere.

use crate::error::HttpError;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

pub(crate) struct Json<T>(pub T);
pub(crate) struct Path<T>(pub T);
pub(crate) struct Query<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Json<T> {
    type Rejection = HttpError;
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        axum::Json::<T>::from_request(request, state)
            .await
            .map(|axum::Json(value)| Self(value))
            .map_err(|error| HttpError::rejected(error.status(), "json"))
    }
}
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Path<T> {
    type Rejection = HttpError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(value)| Self(value))
            .map_err(|error| HttpError::rejected(error.status(), "path"))
    }
}
impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Query<T> {
    type Rejection = HttpError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| Self(value))
            .map_err(|error| HttpError::rejected(error.status(), "query"))
    }
}
