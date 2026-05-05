use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("malformed archive: {0}")]
    Malformed(String),
    #[error("unknown format version {0}")]
    UnknownFormatVersion(u32),
    #[error("schema too new: archive {archive} > current {current}")]
    SchemaTooNew { archive: u32, current: u32 },
    #[error("cross-org payload not allowed")]
    CrossOrgPayload,
    #[error("loro: {0}")]
    Loro(String),
    #[error(transparent)]
    Store(#[from] operon_store::StoreError),
    #[error(transparent)]
    Notes(#[from] operon_notes::NotesError),
}
