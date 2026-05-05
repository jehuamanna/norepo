use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid token")]
    InvalidToken,
    #[error("token expired")]
    Expired,
    #[error("forbidden: {0}")]
    Forbidden(&'static str),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("hash error: {0}")]
    Hash(String),
    #[error("email error: {0}")]
    Email(String),
    #[error(transparent)]
    Store(#[from] operon_store::StoreError),
}
