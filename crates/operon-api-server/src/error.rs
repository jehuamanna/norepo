use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("invalid input: {0}")]
    BadRequest(String),
    #[error("gone")]
    Gone,
    #[error("internal: {0}")]
    Internal(String),
    #[error(transparent)]
    Auth(#[from] operon_auth::AuthError),
    #[error(transparent)]
    Store(#[from] operon_store::StoreError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Conflict(c) => (StatusCode::CONFLICT, *c),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Gone => (StatusCode::GONE, "gone"),
            ApiError::Auth(operon_auth::AuthError::InvalidCredentials) => {
                (StatusCode::UNAUTHORIZED, "invalid_credentials")
            }
            ApiError::Auth(operon_auth::AuthError::InvalidToken) => {
                (StatusCode::BAD_REQUEST, "invalid_token")
            }
            ApiError::Auth(operon_auth::AuthError::Expired) => (StatusCode::GONE, "expired"),
            ApiError::Auth(operon_auth::AuthError::Forbidden(_)) => {
                (StatusCode::FORBIDDEN, "forbidden")
            }
            _ => {
                tracing::error!(err = %self, "internal_error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        let body = Json(json!({ "error": code, "message": self.to_string() }));
        (status, body).into_response()
    }
}
