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
    /// SQL-back-end errors. Available on desktop and on wasm with the
    /// `wasm-sqlite` feature; absent in the wasm-without-feature build
    /// where operon-store ships only error/ids/time helpers.
    #[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
    #[error(transparent)]
    Sqlite(#[from] crate::sql::Error),
    /// Plans-Phase-2-saving: r2d2 connection-pool errors are desktop-only.
    /// The wasm Store guards a single Connection behind a Mutex (no pool).
    #[cfg(not(target_arch = "wasm32"))]
    #[error(transparent)]
    Pool(#[from] r2d2::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl StoreError {
    #[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
    pub fn is_unique_violation(&self) -> bool {
        matches!(
            self,
            StoreError::Sqlite(crate::sql::Error::SqliteFailure(e, _))
                if e.extended_code == crate::sql::ffi::SQLITE_CONSTRAINT_UNIQUE
                || e.extended_code == crate::sql::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        )
    }

    /// Wasm-without-feature stub: there's no SQL back-end, so no unique
    /// violations to detect.
    #[cfg(all(target_arch = "wasm32", not(feature = "wasm-sqlite")))]
    pub fn is_unique_violation(&self) -> bool {
        false
    }
}
