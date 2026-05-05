//! Web (browser) `Persistence` impl.
//!
//! Per locked decision D3: app-sandboxed only. Tries OPFS via
//! `navigator.storage.getDirectory()` first; falls back to IndexedDB if OPFS isn't available
//! (older Safari, Firefox <111). No File System Access API in v1.
//!
//! Compiled only for `target_arch = "wasm32"`.

#![cfg(target_arch = "wasm32")]

use std::future::Future;
use std::pin::Pin;

use js_sys::{Array, JsString, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetFileOptions,
    FileSystemRemoveOptions, FileSystemWritableFileStream, IdbDatabase, IdbObjectStore,
    IdbOpenDbRequest, IdbRequest, IdbTransactionMode,
};

use super::{NoteRef, PersistError, Persistence};

const DB_NAME: &str = "operon-notes";
const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "notes";

/// Combined OPFS + IndexedDB persistence. Resolves the underlying backend at construction
/// time so subsequent calls don't repeat the feature detection.
pub struct WebPersistence {
    backend: WebBackend,
}

enum WebBackend {
    Opfs(OpfsBackend),
    IndexedDb(IndexedDbBackend),
}

impl WebPersistence {
    pub async fn new() -> Result<Self, PersistError> {
        match try_open_opfs().await {
            Ok(root) => Ok(Self { backend: WebBackend::Opfs(OpfsBackend { root }) }),
            Err(_) => {
                let db = open_idb().await?;
                Ok(Self { backend: WebBackend::IndexedDb(IndexedDbBackend { db }) })
            }
        }
    }

    pub fn backend_kind(&self) -> &'static str {
        match self.backend {
            WebBackend::Opfs(_) => "opfs",
            WebBackend::IndexedDb(_) => "indexeddb",
        }
    }
}

// SAFETY: We only ever construct WebPersistence in the wasm single-threaded runtime; the
// inner JsValue handles aren't actually shared across threads. Persistence requires Send +
// Sync, so we mark these explicitly. wasm32-unknown-unknown has no real threading.
unsafe impl Send for WebPersistence {}
unsafe impl Sync for WebPersistence {}

impl Persistence for WebPersistence {
    fn load<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PersistError>> + 'a>> {
        Box::pin(async move {
            match &self.backend {
                WebBackend::Opfs(b) => b.load(note_id).await,
                WebBackend::IndexedDb(b) => b.load(note_id).await,
            }
        })
    }

    fn save<'a>(
        &'a self,
        note_id: &'a str,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            match &self.backend {
                WebBackend::Opfs(b) => b.save(note_id, bytes).await,
                WebBackend::IndexedDb(b) => b.save(note_id, bytes).await,
            }
        })
    }

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteRef>, PersistError>> + 'a>> {
        Box::pin(async move {
            match &self.backend {
                WebBackend::Opfs(b) => b.list().await,
                WebBackend::IndexedDb(b) => b.list().await,
            }
        })
    }

    fn delete<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            match &self.backend {
                WebBackend::Opfs(b) => b.delete(note_id).await,
                WebBackend::IndexedDb(b) => b.delete(note_id).await,
            }
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            // Both impls implement rename as load + save + delete; OPFS doesn't have a
            // single-call rename and IDB doesn't either (key changes are full ops).
            let bytes = self.load(from).await?;
            self.save(to, &bytes).await?;
            self.delete(from).await?;
            Ok(())
        })
    }
}

// =====================================================================================
// OPFS backend
// =====================================================================================

struct OpfsBackend {
    root: FileSystemDirectoryHandle,
}

async fn try_open_opfs() -> Result<FileSystemDirectoryHandle, PersistError> {
    let win = web_sys::window().ok_or_else(|| PersistError::Other("no window".into()))?;
    let storage = win.navigator().storage();
    let promise = storage.get_directory();
    let handle = JsFuture::from(promise)
        .await
        .map_err(|e| PersistError::Other(format!("opfs unavailable: {e:?}")))?;
    handle
        .dyn_into::<FileSystemDirectoryHandle>()
        .map_err(|_| PersistError::Other("opfs returned wrong handle".into()))
}

impl OpfsBackend {
    async fn get_file(
        &self,
        note_id: &str,
        create: bool,
    ) -> Result<FileSystemFileHandle, PersistError> {
        let opts = FileSystemGetFileOptions::new();
        opts.set_create(create);
        let promise = self.root.get_file_handle_with_options(note_id, &opts);
        let h = JsFuture::from(promise).await.map_err(map_dom_err)?;
        h.dyn_into::<FileSystemFileHandle>()
            .map_err(|_| PersistError::Other("file handle cast failed".into()))
    }

    async fn load(&self, note_id: &str) -> Result<Vec<u8>, PersistError> {
        let file_handle = self.get_file(note_id, false).await?;
        let file = JsFuture::from(file_handle.get_file()).await.map_err(map_dom_err)?;
        let blob: web_sys::Blob = file
            .dyn_into()
            .map_err(|_| PersistError::Other("file is not Blob".into()))?;
        let buf = JsFuture::from(blob.array_buffer()).await.map_err(map_dom_err)?;
        let array = Uint8Array::new(&buf);
        let mut out = vec![0u8; array.length() as usize];
        array.copy_to(&mut out);
        Ok(out)
    }

    async fn save(&self, note_id: &str, bytes: &[u8]) -> Result<(), PersistError> {
        let file_handle = self.get_file(note_id, true).await?;
        let writable_promise = file_handle.create_writable();
        let writable_js = JsFuture::from(writable_promise)
            .await
            .map_err(map_dom_err)?;
        let writable: FileSystemWritableFileStream = writable_js
            .dyn_into()
            .map_err(|_| PersistError::Other("writable cast failed".into()))?;
        let buf = Uint8Array::from(bytes);
        let write_promise = writable
            .write_with_buffer_source(&buf)
            .map_err(map_dom_err)?;
        JsFuture::from(write_promise).await.map_err(map_dom_err)?;
        JsFuture::from(writable.close()).await.map_err(map_dom_err)?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<NoteRef>, PersistError> {
        // OPFS exposes entries via async iteration. We use the entries() call directly via
        // Reflect, since web_sys doesn't bind it across all browsers identically.
        let entries_fn = Reflect::get(&self.root, &JsValue::from_str("entries"))
            .map_err(|_| PersistError::Other("entries() missing".into()))?;
        let func = entries_fn
            .dyn_into::<js_sys::Function>()
            .map_err(|_| PersistError::Other("entries() not callable".into()))?;
        let iter = func
            .call0(&self.root)
            .map_err(map_dom_err)?;
        let next_fn = Reflect::get(&iter, &JsValue::from_str("next"))
            .map_err(|_| PersistError::Other("iter.next missing".into()))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| PersistError::Other("iter.next not callable".into()))?;
        let mut out = Vec::new();
        loop {
            let next_promise = next_fn
                .call0(&iter)
                .map_err(map_dom_err)?
                .dyn_into::<js_sys::Promise>()
                .map_err(|_| PersistError::Other("iter.next not a Promise".into()))?;
            let result = JsFuture::from(next_promise).await.map_err(map_dom_err)?;
            let done = Reflect::get(&result, &JsValue::from_str("done"))
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if done {
                break;
            }
            let value = Reflect::get(&result, &JsValue::from_str("value"))
                .map_err(map_dom_err)?;
            // value is [name, handle]
            let arr: Array = value
                .dyn_into()
                .map_err(|_| PersistError::Other("entry not array".into()))?;
            let name = arr.get(0).as_string().unwrap_or_default();
            if name.starts_with('.') || name == SEEDED_MARKER {
                continue;
            }
            let format_id = name.rsplit_once('.').map(|(_, ext)| ext.to_string());
            out.push(NoteRef { note_id: name, format_id, last_modified_ms: None });
        }
        Ok(out)
    }

    async fn delete(&self, note_id: &str) -> Result<(), PersistError> {
        let opts = FileSystemRemoveOptions::new();
        let promise = self.root.remove_entry_with_options(note_id, &opts);
        JsFuture::from(promise).await.map_err(map_dom_err)?;
        Ok(())
    }
}

// =====================================================================================
// IndexedDB backend
// =====================================================================================

struct IndexedDbBackend {
    db: IdbDatabase,
}

async fn open_idb() -> Result<IdbDatabase, PersistError> {
    let win = web_sys::window().ok_or_else(|| PersistError::Other("no window".into()))?;
    let factory = win
        .indexed_db()
        .map_err(map_dom_err)?
        .ok_or_else(|| PersistError::Other("indexedDB unavailable".into()))?;
    let req: IdbOpenDbRequest = factory
        .open_with_u32(DB_NAME, DB_VERSION)
        .map_err(map_dom_err)?;
    // Set up upgrade handler — creates the object store on first open.
    let upgrade_cb = wasm_bindgen::closure::Closure::wrap(Box::new(
        move |evt: web_sys::IdbVersionChangeEvent| {
            let target = evt.target().unwrap();
            let req: IdbOpenDbRequest = target.dyn_into().unwrap();
            let db: IdbDatabase = req.result().unwrap().dyn_into().unwrap();
            // Skip the existence check — create_object_store will throw if the store
            // already exists, but on `onupgradeneeded` for a new version that's not
            // possible (the upgrade transaction creates fresh stores). For v1 we just
            // attempt creation and ignore the error.
            let _ = db.create_object_store(STORE_NAME);
        },
    )
        as Box<dyn FnMut(web_sys::IdbVersionChangeEvent)>);
    req.set_onupgradeneeded(Some(upgrade_cb.as_ref().unchecked_ref()));
    upgrade_cb.forget(); // keep alive for the lifetime of this open

    request_to_future(&req).await?;
    let db: IdbDatabase = req
        .result()
        .map_err(map_dom_err)?
        .dyn_into()
        .map_err(|_| PersistError::Other("idb result not IdbDatabase".into()))?;
    Ok(db)
}

impl IndexedDbBackend {
    fn store(&self, mode: IdbTransactionMode) -> Result<IdbObjectStore, PersistError> {
        let store_names = Array::new();
        store_names.push(&JsValue::from_str(STORE_NAME));
        let tx = self
            .db
            .transaction_with_str_sequence_and_mode(&store_names, mode)
            .map_err(map_dom_err)?;
        tx.object_store(STORE_NAME).map_err(map_dom_err)
    }

    async fn load(&self, note_id: &str) -> Result<Vec<u8>, PersistError> {
        let store = self.store(IdbTransactionMode::Readonly)?;
        let req = store.get(&JsValue::from_str(note_id)).map_err(map_dom_err)?;
        request_to_future(&req).await?;
        let value = req.result().map_err(map_dom_err)?;
        if value.is_undefined() {
            return Err(PersistError::NotFound);
        }
        // value shape: { bytes: Uint8Array, format_id?: string }
        let buf = Reflect::get(&value, &JsValue::from_str("bytes"))
            .map_err(|_| PersistError::Other("idb record missing bytes".into()))?;
        let array: Uint8Array = buf
            .dyn_into()
            .map_err(|_| PersistError::Other("idb bytes not Uint8Array".into()))?;
        let mut out = vec![0u8; array.length() as usize];
        array.copy_to(&mut out);
        Ok(out)
    }

    async fn save(&self, note_id: &str, bytes: &[u8]) -> Result<(), PersistError> {
        let store = self.store(IdbTransactionMode::Readwrite)?;
        let value = Object::new();
        let arr = Uint8Array::from(bytes);
        let _ = Reflect::set(&value, &JsValue::from_str("bytes"), &arr);
        let format_id = note_id.rsplit_once('.').map(|(_, ext)| ext);
        if let Some(ext) = format_id {
            let _ = Reflect::set(
                &value,
                &JsValue::from_str("format_id"),
                &JsValue::from_str(ext),
            );
        }
        let req = store
            .put_with_key(&value, &JsValue::from_str(note_id))
            .map_err(map_dom_err)?;
        request_to_future(&req).await?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<NoteRef>, PersistError> {
        let store = self.store(IdbTransactionMode::Readonly)?;
        let req = store.get_all_keys().map_err(map_dom_err)?;
        request_to_future(&req).await?;
        let keys: Array = req
            .result()
            .map_err(map_dom_err)?
            .dyn_into()
            .map_err(|_| PersistError::Other("getAllKeys not array".into()))?;
        let mut out = Vec::new();
        for i in 0..keys.length() {
            let key = keys.get(i);
            if let Some(s) = key.dyn_ref::<JsString>().and_then(|js| js.as_string()) {
                if s.starts_with('.') || s == SEEDED_MARKER {
                    continue;
                }
                let format_id = s.rsplit_once('.').map(|(_, ext)| ext.to_string());
                out.push(NoteRef { note_id: s, format_id, last_modified_ms: None });
            }
        }
        Ok(out)
    }

    async fn delete(&self, note_id: &str) -> Result<(), PersistError> {
        let store = self.store(IdbTransactionMode::Readwrite)?;
        let req = store.delete(&JsValue::from_str(note_id)).map_err(map_dom_err)?;
        request_to_future(&req).await?;
        Ok(())
    }
}

// =====================================================================================
// Helpers
// =====================================================================================

const SEEDED_MARKER: &str = "__seeded__";

/// Wrap an `IdbRequest` in a future that resolves on `success` and rejects on `error`. The
/// closures are leaked via `forget` because the request goes out of scope right after this
/// helper completes — the JS engine's GC reaps them.
fn request_to_future(
    req: &IdbRequest,
) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + '_>> {
    Box::pin(async move {
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let resolve = resolve.clone();
            let reject = reject.clone();
            let success_cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                let _ = resolve.call0(&JsValue::NULL);
            });
            let error_cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                let _ = reject.call0(&JsValue::NULL);
            });
            req.set_onsuccess(Some(success_cb.unchecked_ref()));
            req.set_onerror(Some(error_cb.unchecked_ref()));
        });
        JsFuture::from(promise).await.map_err(map_dom_err)?;
        Ok(())
    })
}

fn map_dom_err(e: JsValue) -> PersistError {
    PersistError::Io(format!("{e:?}"))
}
