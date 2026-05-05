use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid uuid: {0}")]
    InvalidUuid(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("open: {0}")]
    Open(String),
    #[error("migrate: {0}")]
    Migrate(String),
    #[error("unknown applied migration version {0}")]
    UnknownAppliedVersion(i64),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Pool(#[from] r2d2::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl StoreError {
    pub fn is_unique_violation(&self) -> bool {
        matches!(
            self,
            StoreError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                || e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        )
    }
}
