//! Note persistence — bytes in, bytes out.
//!
//! [`Persistence`] is the storage abstraction every format plugin sits behind. It is bytes-only
//! by design: frontmatter, link extraction, JSON parsing — those are the format plugin's job,
//! not storage's. Implementations are cfg-gated:
//! - desktop: [`fs::FilesystemPersistence`] (atomic write-temp + rename, `notify` for watch).
//! - wasm: a future `WebPersistence` (OPFS first, IndexedDB fallback) lands in Phase 3.
//!
//! [`NoteWatcher`] is a separate trait so that backends without change-notification (web) can
//! cleanly opt out without wedging an `Option<Stream<...>>` into the main `Persistence`
//! contract.

use std::future::Future;
use std::pin::Pin;

#[cfg(not(target_arch = "wasm32"))]
pub mod fs;
pub mod memory;
#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use fs::FilesystemPersistence;
pub use memory::MemoryPersistence;
#[cfg(target_arch = "wasm32")]
pub use web::WebPersistence;

/// Lightweight reference returned by `list()`. The `format_id` is captured at write-time when
/// the backend can determine it (e.g., from filename extension on disk).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRef {
    pub note_id: String,
    pub format_id: Option<String>,
    pub last_modified_ms: Option<u64>,
}

/// Storage error surface. `NotFound` is the only variant callers should match-on; everything
/// else is logged or surfaced to the user as "save failed — see logs".
#[derive(Debug)]
pub enum PersistError {
    NotFound,
    Io(String),
    Other(String),
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "note not found"),
            Self::Io(msg) => write!(f, "i/o error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for PersistError {}

/// Bytes-only storage trait. All methods return owned futures so trait objects work cleanly
/// across desktop sync FS calls and wasm async OPFS/IDB calls.
///
/// The future returns are NOT `Send` because the wasm `WebPersistence` impl holds JsValue
/// handles (which are `!Send`). Dioxus's `spawn` runs on the local thread so this is fine;
/// any cross-thread storage work happens via background tasks queued elsewhere.
pub trait Persistence: Send + Sync {
    fn load<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PersistError>> + 'a>>;

    fn save<'a>(
        &'a self,
        note_id: &'a str,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>>;

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteRef>, PersistError>> + 'a>>;

    fn delete<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>>;

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>>;
}

/// External-change notification. Desktop FS impl uses the `notify` crate; web impl is a no-op
/// (OPFS has no change events, and we don't share storage cross-tab in v1).
#[derive(Clone, Debug)]
pub enum WatchEvent {
    Modified(String),
    Created(String),
    Removed(String),
    Renamed { from: String, to: String },
}

/// Returned by `subscribe`; dropping it cancels the subscription. Desktop impl owns a
/// `notify::RecommendedWatcher` here; web impl is a zero-sized stub.
pub struct WatchHandle {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) _inner: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl WatchHandle {
    /// A handle that holds nothing — used by the wasm no-op impl and by tests.
    pub fn empty() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            _inner: None,
        }
    }
}

pub trait NoteWatcher: Send + Sync {
    /// Subscribe to change events for the storage root. The returned [`WatchHandle`] keeps the
    /// subscription alive until dropped.
    fn subscribe(&self, cb: Box<dyn Fn(WatchEvent) + Send + Sync>) -> WatchHandle;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_ref_eq_is_structural() {
        let a = NoteRef {
            note_id: "n1".into(),
            format_id: Some("markdown".into()),
            last_modified_ms: Some(1_700_000_000_000),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn persist_error_displays() {
        assert_eq!(format!("{}", PersistError::NotFound), "note not found");
        assert_eq!(
            format!("{}", PersistError::Io("disk full".into())),
            "i/o error: disk full"
        );
    }

    #[test]
    fn watch_handle_empty_constructs() {
        let _h = WatchHandle::empty();
    }
}
