//! Plans-Phase-2-saving / Option 2: OPFS-backed [`Persistence`] for the
//! web build.
//!
//! Mirrors the desktop `FilesystemPersistence` shape (atomic write via
//! tempfile + rename, async `load`/`save`/`list`/`delete`/`rename`) over
//! the OPFS APIs exposed by the browser. Opens a `FileSystemDirectoryHandle`
//! pointing at `<vault>/notes/` and writes one file per note id.
//!
//! No Worker is required for v1: we use `FileSystemFileHandle::createWritable`
//! (the async writable-stream path), which works on the main thread and
//! doesn't need SharedArrayBuffer / cross-origin isolation.
//!
//! Activated only on wasm builds with `--features wasm-sqlite`. The
//! Local-Mode shell wires this in as a replacement for
//! `wasm_stub::MemoryPersistence` once the rest of Phase E lands.

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

use std::future::Future;
use std::pin::Pin;

use js_sys::{Array, ArrayBuffer, JsString, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetFileOptions,
    FileSystemRemoveOptions, FileSystemWritableFileStream,
};

use super::{NoteRef, PersistError, Persistence};

/// OPFS-backed persistence rooted at `<vault>/notes/`. Cloning is cheap;
/// the underlying handle is reference-counted via `JsValue`.
#[derive(Clone)]
pub struct OpfsPersistence {
    notes_dir: FileSystemDirectoryHandle,
}

impl OpfsPersistence {
    /// Construct from an already-resolved `<vault>/notes/` handle. The
    /// caller (Local-Mode boot) walks `vault_handle.getDirectoryHandle("notes",
    /// {create: true})` to produce this argument.
    pub fn new(notes_dir: FileSystemDirectoryHandle) -> Self {
        Self { notes_dir }
    }
}

fn js_err(e: JsValue) -> PersistError {
    let msg = if let Some(s) = e.as_string() {
        s
    } else if let Ok(s) = Reflect::get(&e, &JsString::from("message")) {
        s.as_string().unwrap_or_else(|| format!("{e:?}"))
    } else {
        format!("{e:?}")
    };
    PersistError::Io(msg)
}

async fn get_file_handle(
    dir: &FileSystemDirectoryHandle,
    name: &str,
    create: bool,
) -> Result<FileSystemFileHandle, PersistError> {
    let opts = FileSystemGetFileOptions::new();
    opts.set_create(create);
    let promise = dir.get_file_handle_with_options(name, &opts);
    let value = JsFuture::from(promise).await.map_err(js_err)?;
    value
        .dyn_into::<FileSystemFileHandle>()
        .map_err(|v| PersistError::Io(format!("not a FileSystemFileHandle: {v:?}")))
}

async fn read_handle_bytes(handle: &FileSystemFileHandle) -> Result<Vec<u8>, PersistError> {
    let file_value = JsFuture::from(handle.get_file()).await.map_err(js_err)?;
    let blob: Blob = file_value
        .dyn_into()
        .map_err(|v| PersistError::Io(format!("not a Blob: {v:?}")))?;
    let buf_value = JsFuture::from(blob.array_buffer()).await.map_err(js_err)?;
    let buf: ArrayBuffer = buf_value
        .dyn_into()
        .map_err(|v| PersistError::Io(format!("not an ArrayBuffer: {v:?}")))?;
    let view = Uint8Array::new(&buf);
    Ok(view.to_vec())
}

async fn write_handle_bytes(
    handle: &FileSystemFileHandle,
    bytes: &[u8],
) -> Result<(), PersistError> {
    let writable_value = JsFuture::from(handle.create_writable())
        .await
        .map_err(js_err)?;
    let writable: FileSystemWritableFileStream = writable_value
        .dyn_into()
        .map_err(|v| PersistError::Io(format!("not a FileSystemWritableFileStream: {v:?}")))?;
    let view = Uint8Array::from(bytes);
    let promise = writable
        .write_with_buffer_source(&view.buffer())
        .map_err(js_err)?;
    JsFuture::from(promise).await.map_err(js_err)?;
    JsFuture::from(writable.close()).await.map_err(js_err)?;
    Ok(())
}

impl Persistence for OpfsPersistence {
    fn load<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PersistError>> + 'a>> {
        Box::pin(async move {
            match get_file_handle(&self.notes_dir, note_id, false).await {
                Ok(h) => read_handle_bytes(&h).await,
                Err(PersistError::Io(s)) if s.contains("NotFoundError") => {
                    Err(PersistError::NotFound)
                }
                Err(e) => Err(e),
            }
        })
    }

    fn save<'a>(
        &'a self,
        note_id: &'a str,
        bytes: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            let h = get_file_handle(&self.notes_dir, note_id, true).await?;
            write_handle_bytes(&h, bytes).await
        })
    }

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteRef>, PersistError>> + 'a>> {
        Box::pin(async move {
            // FileSystemDirectoryHandle.entries() returns an async iterator
            // of [name, handle] pairs. We iterate via the symbol-async-iterator
            // protocol exposed through Reflect.
            let mut out = Vec::new();
            let entries_method = Reflect::get(&self.notes_dir, &JsString::from("entries"))
                .map_err(js_err)?;
            let func: js_sys::Function = entries_method
                .dyn_into()
                .map_err(|v| PersistError::Io(format!("entries() not a function: {v:?}")))?;
            let iter_obj: Object = func
                .call0(&self.notes_dir)
                .map_err(js_err)?
                .dyn_into()
                .map_err(|v| PersistError::Io(format!("entries() not an object: {v:?}")))?;
            let next_method = Reflect::get(&iter_obj, &JsString::from("next"))
                .map_err(js_err)?
                .dyn_into::<js_sys::Function>()
                .map_err(|v| PersistError::Io(format!("next() not a function: {v:?}")))?;
            loop {
                let next_promise = next_method.call0(&iter_obj).map_err(js_err)?;
                let result = JsFuture::from(js_sys::Promise::from(next_promise))
                    .await
                    .map_err(js_err)?;
                let done = Reflect::get(&result, &JsString::from("done"))
                    .map_err(js_err)?
                    .as_bool()
                    .unwrap_or(false);
                if done {
                    break;
                }
                let value = Reflect::get(&result, &JsString::from("value"))
                    .map_err(js_err)?;
                let pair: Array = value
                    .dyn_into()
                    .map_err(|v| PersistError::Io(format!("entry not an array: {v:?}")))?;
                let name_js = pair.get(0);
                let name = name_js.as_string().unwrap_or_default();
                if name.starts_with('.') {
                    continue;
                }
                out.push(NoteRef {
                    note_id: name.clone(),
                    format_id: std::path::Path::new(&name)
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string()),
                    last_modified_ms: None,
                });
            }
            Ok(out)
        })
    }

    fn delete<'a>(
        &'a self,
        note_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            let opts = FileSystemRemoveOptions::new();
            let promise = self
                .notes_dir
                .remove_entry_with_options(note_id, &opts);
            match JsFuture::from(promise).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    let msg = format!("{e:?}");
                    if msg.contains("NotFoundError") {
                        Err(PersistError::NotFound)
                    } else {
                        Err(PersistError::Io(msg))
                    }
                }
            }
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PersistError>> + 'a>> {
        Box::pin(async move {
            // OPFS doesn't expose a native rename; copy bytes + delete the
            // source. For note-body-sized blobs this is fine; image blobs
            // are already content-addressed so they don't need rename.
            let bytes = match get_file_handle(&self.notes_dir, from, false).await {
                Ok(h) => read_handle_bytes(&h).await?,
                Err(PersistError::Io(s)) if s.contains("NotFoundError") => {
                    return Err(PersistError::NotFound);
                }
                Err(e) => return Err(e),
            };
            let dst = get_file_handle(&self.notes_dir, to, true).await?;
            write_handle_bytes(&dst, &bytes).await?;
            let opts = FileSystemRemoveOptions::new();
            let _ = JsFuture::from(self.notes_dir.remove_entry_with_options(from, &opts)).await;
            Ok(())
        })
    }
}
