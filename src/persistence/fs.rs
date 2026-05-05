//! Desktop filesystem-backed [`Persistence`].
//!
//! Atomic save uses `tempfile::NamedTempFile::persist`: write a sibling tempfile, then rename
//! into place. On POSIX `rename(2)` is atomic; on Windows the crate uses
//! `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`. A crash mid-rename leaves either the old
//! file (if the rename hasn't started) or the new file (if it has) — never a partial write.
//!
//! `notify`-based [`NoteWatcher`] surfaces external file changes to the app. The web build
//! ships a no-op watcher (OPFS lacks change events).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use super::{NoteRef, NoteWatcher, PersistError, Persistence, WatchEvent, WatchHandle};

/// Filesystem-backed `Persistence`. Notes live as files under `notes_dir` named `<note_id>` —
/// the format plugin owns extension semantics (the plugin's resolver maps file extension to
/// `format_id` at open-time; this layer is bytes-only).
#[derive(Clone)]
pub struct FilesystemPersistence {
    notes_dir: Arc<PathBuf>,
}

impl FilesystemPersistence {
    /// Construct a new persistence rooted at `notes_dir`. Creates the directory if missing.
    pub fn new(notes_dir: impl Into<PathBuf>) -> Result<Self, PersistError> {
        let path: PathBuf = notes_dir.into();
        std::fs::create_dir_all(&path).map_err(|e| PersistError::Io(e.to_string()))?;
        Ok(Self { notes_dir: Arc::new(path) })
    }

    fn path_for(&self, note_id: &str) -> PathBuf {
        self.notes_dir.join(note_id)
    }
}

impl Persistence for FilesystemPersistence {
    fn load<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PersistError>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.path_for(note_id);
            match std::fs::read(&path) {
                Ok(bytes) => Ok(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(PersistError::NotFound),
                Err(e) => Err(PersistError::Io(e.to_string())),
            }
        })
    }

    fn save<'a>(
        &'a self,
        note_id: &'a str,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + Send + 'a>> {
        Box::pin(async move {
            let final_path = self.path_for(note_id);
            let temp = tempfile::NamedTempFile::new_in(self.notes_dir.as_path())
                .map_err(|e| PersistError::Io(e.to_string()))?;
            std::fs::write(temp.path(), bytes).map_err(|e| PersistError::Io(e.to_string()))?;
            temp.persist(&final_path)
                .map_err(|e| PersistError::Io(e.error.to_string()))?;
            Ok(())
        })
    }

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteRef>, PersistError>> + Send + 'a>> {
        Box::pin(async move {
            let mut out = Vec::new();
            let dir = std::fs::read_dir(self.notes_dir.as_path())
                .map_err(|e| PersistError::Io(e.to_string()))?;
            for entry in dir {
                let entry = entry.map_err(|e| PersistError::Io(e.to_string()))?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let note_id = match path.file_name().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if note_id.starts_with('.') {
                    continue; // hidden files (e.g., __seeded__ marker — used by web only)
                }
                let format_id = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string());
                let last_modified_ms = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64);
                out.push(NoteRef { note_id, format_id, last_modified_ms });
            }
            Ok(out)
        })
    }

    fn delete<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.path_for(note_id);
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(PersistError::NotFound),
                Err(e) => Err(PersistError::Io(e.to_string())),
            }
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + Send + 'a>> {
        Box::pin(async move {
            let from_path = self.path_for(from);
            let to_path = self.path_for(to);
            match std::fs::rename(&from_path, &to_path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(PersistError::NotFound),
                Err(e) => Err(PersistError::Io(e.to_string())),
            }
        })
    }
}

/// `notify`-backed change watcher for `FilesystemPersistence`. The watcher boxes its
/// `RecommendedWatcher` and stores it in the returned [`WatchHandle`]; dropping the handle
/// stops the subscription.
pub struct FilesystemWatcher {
    notes_dir: Arc<PathBuf>,
}

impl FilesystemWatcher {
    pub fn new(notes_dir: impl Into<PathBuf>) -> Self {
        Self { notes_dir: Arc::new(notes_dir.into()) }
    }
}

impl NoteWatcher for FilesystemWatcher {
    fn subscribe(&self, cb: Box<dyn Fn(WatchEvent) + Send + Sync>) -> WatchHandle {
        use notify::{recommended_watcher, EventKind, RecursiveMode, Watcher};
        let dir = self.notes_dir.clone();
        let cb = Arc::new(cb);
        let cb_for_event = cb.clone();
        let watcher_result = recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            for path in &ev.paths {
                let id = match path.file_name().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let event = match ev.kind {
                    EventKind::Create(_) => WatchEvent::Created(id),
                    EventKind::Modify(_) => WatchEvent::Modified(id),
                    EventKind::Remove(_) => WatchEvent::Removed(id),
                    _ => continue,
                };
                cb_for_event(event);
            }
        });
        let mut watcher = match watcher_result {
            Ok(w) => w,
            Err(_) => return WatchHandle::empty(),
        };
        if watcher.watch(dir.as_path(), RecursiveMode::NonRecursive).is_err() {
            return WatchHandle::empty();
        }
        WatchHandle { _inner: Some(Box::new(watcher)) }
    }
}

#[allow(dead_code)]
fn assert_send_sync<T: Send + Sync>() {}
#[allow(dead_code)]
fn _assert_traits() {
    assert_send_sync::<FilesystemPersistence>();
    assert_send_sync::<FilesystemWatcher>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, FilesystemPersistence) {
        let tmp = TempDir::new().unwrap();
        let p = FilesystemPersistence::new(tmp.path()).unwrap();
        (tmp, p)
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        // Tiny single-threaded executor for tests — avoids a tokio dep.
        use std::task::Context;
        use std::task::Poll;
        let mut f = Box::pin(f);
        let waker = futures_waker();
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    fn futures_waker() -> std::task::Waker {
        use std::task::{RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| (),
            |_| (),
            |_| (),
        );
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    #[test]
    fn save_load_roundtrip() {
        let (_tmp, p) = fixture();
        block_on(p.save("note-a", b"hello world")).unwrap();
        let bytes = block_on(p.load("note-a")).unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn load_nonexistent_returns_not_found() {
        let (_tmp, p) = fixture();
        let err = block_on(p.load("missing")).unwrap_err();
        assert!(matches!(err, PersistError::NotFound));
    }

    #[test]
    fn save_overwrites_existing() {
        let (_tmp, p) = fixture();
        block_on(p.save("n", b"first")).unwrap();
        block_on(p.save("n", b"second")).unwrap();
        assert_eq!(block_on(p.load("n")).unwrap(), b"second");
    }

    #[test]
    fn list_enumerates_saved_notes() {
        let (_tmp, p) = fixture();
        block_on(p.save("a.md", b"x")).unwrap();
        block_on(p.save("b.json", b"{}")).unwrap();
        let mut refs = block_on(p.list()).unwrap();
        refs.sort_by(|a, b| a.note_id.cmp(&b.note_id));
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].note_id, "a.md");
        assert_eq!(refs[0].format_id.as_deref(), Some("md"));
        assert_eq!(refs[1].note_id, "b.json");
        assert_eq!(refs[1].format_id.as_deref(), Some("json"));
    }

    #[test]
    fn delete_removes_file() {
        let (_tmp, p) = fixture();
        block_on(p.save("n", b"x")).unwrap();
        block_on(p.delete("n")).unwrap();
        assert!(matches!(
            block_on(p.load("n")).unwrap_err(),
            PersistError::NotFound
        ));
    }

    #[test]
    fn delete_nonexistent_returns_not_found() {
        let (_tmp, p) = fixture();
        assert!(matches!(
            block_on(p.delete("ghost")).unwrap_err(),
            PersistError::NotFound
        ));
    }

    #[test]
    fn rename_moves_content() {
        let (_tmp, p) = fixture();
        block_on(p.save("old", b"abc")).unwrap();
        block_on(p.rename("old", "new")).unwrap();
        assert!(matches!(
            block_on(p.load("old")).unwrap_err(),
            PersistError::NotFound
        ));
        assert_eq!(block_on(p.load("new")).unwrap(), b"abc");
    }

    #[test]
    fn list_skips_hidden_files() {
        let (_tmp, p) = fixture();
        block_on(p.save("visible", b"x")).unwrap();
        std::fs::write(p.notes_dir.join(".hidden"), b"x").unwrap();
        let refs = block_on(p.list()).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].note_id, "visible");
    }

    #[test]
    fn atomic_save_no_partial_write_on_overwrite() {
        // Smoke test: rapid back-to-back saves on the same id end with the last write's bytes
        // and never produce a half-written file. Atomicity guaranteed by tempfile::persist.
        let (_tmp, p) = fixture();
        for i in 0..50u32 {
            block_on(p.save("n", format!("payload-{i}").as_bytes())).unwrap();
        }
        assert_eq!(block_on(p.load("n")).unwrap(), b"payload-49");
    }

    #[test]
    fn unused_send_sync_assertion_compiles() {
        _assert_traits();
    }
}
