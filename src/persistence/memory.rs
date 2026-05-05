//! In-memory `Persistence` impl. Used as the wasm-side default until Phase 3 lands the real
//! `WebPersistence` (OPFS + IndexedDB fallback). Also handy in tests as a stub.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use super::{NoteRef, PersistError, Persistence};

#[derive(Default)]
pub struct MemoryPersistence {
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryPersistence {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Persistence for MemoryPersistence {
    fn load<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PersistError>> + 'a>> {
        Box::pin(async move {
            let store = self.inner.lock().map_err(|e| PersistError::Other(e.to_string()))?;
            store.get(note_id).cloned().ok_or(PersistError::NotFound)
        })
    }

    fn save<'a>(
        &'a self,
        note_id: &'a str,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            let mut store = self.inner.lock().map_err(|e| PersistError::Other(e.to_string()))?;
            store.insert(note_id.to_string(), bytes.to_vec());
            Ok(())
        })
    }

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteRef>, PersistError>> + 'a>> {
        Box::pin(async move {
            let store = self.inner.lock().map_err(|e| PersistError::Other(e.to_string()))?;
            Ok(store
                .keys()
                .map(|k| NoteRef {
                    note_id: k.clone(),
                    format_id: k.rsplit('.').next().filter(|_| k.contains('.')).map(String::from),
                    last_modified_ms: None,
                })
                .collect())
        })
    }

    fn delete<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            let mut store = self.inner.lock().map_err(|e| PersistError::Other(e.to_string()))?;
            store.remove(note_id).map(|_| ()).ok_or(PersistError::NotFound)
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            let mut store = self.inner.lock().map_err(|e| PersistError::Other(e.to_string()))?;
            let bytes = store.remove(from).ok_or(PersistError::NotFound)?;
            store.insert(to.to_string(), bytes);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    fn block_on<F: Future>(f: F) -> F::Output {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| (),
            |_| (),
            |_| (),
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut f = Box::pin(f);
        loop {
            if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let p = MemoryPersistence::new();
        block_on(p.save("a", b"hello")).unwrap();
        assert_eq!(block_on(p.load("a")).unwrap(), b"hello");
    }

    #[test]
    fn load_missing_is_not_found() {
        let p = MemoryPersistence::new();
        assert!(matches!(
            block_on(p.load("ghost")).unwrap_err(),
            PersistError::NotFound
        ));
    }

    #[test]
    fn rename_moves_bytes() {
        let p = MemoryPersistence::new();
        block_on(p.save("from", b"x")).unwrap();
        block_on(p.rename("from", "to")).unwrap();
        assert_eq!(block_on(p.load("to")).unwrap(), b"x");
        assert!(matches!(
            block_on(p.load("from")).unwrap_err(),
            PersistError::NotFound
        ));
    }
}
