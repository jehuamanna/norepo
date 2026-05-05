use thiserror::Error;

#[derive(Debug, Error)]
pub enum NotesError {
    #[error("frame decode: {0}")]
    Frame(String),
    #[error("loro: {0}")]
    Loro(String),
    #[error(transparent)]
    Store(#[from] operon_store::StoreError),
}
