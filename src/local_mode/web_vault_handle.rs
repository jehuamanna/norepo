//! IndexedDB persistence for the user's chosen OPFS `FileSystemDirectoryHandle`.
//!
//! Wasm-only. The handle is opaque to JS but `structuredClone`-cloneable, so
//! we can store it in an IDB row and read it back on the next page load —
//! letting the picker prompt the user once, then transparently reuse the
//! choice across reloads. Consumed by Phase 2's web persistence work.

#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransactionMode,
};

const DB_NAME: &str = "operon";
const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "vault-handle";
const KEY_CURRENT: &str = "current";

#[derive(Debug)]
pub enum WebVaultErr {
    NoIndexedDb,
    Open(String),
    Tx(String),
    Other(String),
}

impl std::fmt::Display for WebVaultErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoIndexedDb => f.write_str("indexedDB unavailable in this context"),
            Self::Open(s) => write!(f, "indexedDB open failed: {s}"),
            Self::Tx(s) => write!(f, "indexedDB transaction failed: {s}"),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for WebVaultErr {}

/// Persist a directory handle under the well-known `current` key.
pub async fn store_handle(handle: &FileSystemDirectoryHandle) -> Result<(), WebVaultErr> {
    let db = open_db().await?;
    let tx = db
        .transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readwrite)
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let store = tx
        .object_store(STORE_NAME)
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let req = store
        .put_with_key(handle.as_ref(), &JsValue::from_str(KEY_CURRENT))
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    await_request(&req).await
}

/// Load the previously stored handle, or `None` if no row exists yet.
pub async fn load_handle() -> Result<Option<FileSystemDirectoryHandle>, WebVaultErr> {
    let db = open_db().await?;
    let tx = db
        .transaction_with_str(STORE_NAME)
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let store = tx
        .object_store(STORE_NAME)
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let req = store
        .get(&JsValue::from_str(KEY_CURRENT))
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let value = await_request_with_result(&req).await?;
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    Ok(Some(value.unchecked_into::<FileSystemDirectoryHandle>()))
}

/// Remove the stored handle. Used by Settings → "Change…" before re-prompting.
pub async fn clear_handle() -> Result<(), WebVaultErr> {
    let db = open_db().await?;
    let tx = db
        .transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readwrite)
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let store = tx
        .object_store(STORE_NAME)
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    let req = store
        .delete(&JsValue::from_str(KEY_CURRENT))
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))?;
    await_request(&req).await
}

async fn open_db() -> Result<IdbDatabase, WebVaultErr> {
    let window = web_sys::window().ok_or(WebVaultErr::NoIndexedDb)?;
    let factory = window
        .indexed_db()
        .map_err(|e| WebVaultErr::Open(format_js(&e)))?
        .ok_or(WebVaultErr::NoIndexedDb)?;
    let req: IdbOpenDbRequest = factory
        .open_with_u32(DB_NAME, DB_VERSION)
        .map_err(|e| WebVaultErr::Open(format_js(&e)))?;

    // Create the store on first open / version bump.
    let req_clone = req.clone();
    let onupgrade: Closure<dyn FnMut(JsValue)> = Closure::new(move |_evt: JsValue| {
        if let Ok(db) = req_clone
            .result()
            .and_then(|v| v.dyn_into::<IdbDatabase>().map_err(|_| JsValue::NULL))
        {
            // Idempotent: create_object_store fails if the store exists; we
            // explicitly check first via DOMStringList.contains.
            let names = db.object_store_names();
            if !names.contains(STORE_NAME) {
                let _ = db.create_object_store(STORE_NAME);
            }
        }
    });
    req.set_onupgradeneeded(Some(onupgrade.as_ref().unchecked_ref()));
    onupgrade.forget();

    let promise = Promise::new(&mut |resolve, reject| {
        let req2 = req.clone();
        let resolve_for_success = resolve.clone();
        let onsuccess: Closure<dyn FnMut(JsValue)> = Closure::new(move |_evt: JsValue| {
            let v = req2.result().unwrap_or(JsValue::NULL);
            let _ = resolve_for_success.call1(&JsValue::NULL, &v);
        });
        req.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
        onsuccess.forget();

        let onerror: Closure<dyn FnMut(JsValue)> = Closure::new(move |_evt: JsValue| {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("idb open onerror"));
        });
        req.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
    });
    let v = JsFuture::from(promise)
        .await
        .map_err(|e| WebVaultErr::Open(format_js(&e)))?;
    v.dyn_into::<IdbDatabase>()
        .map_err(|_| WebVaultErr::Open("open returned non-IdbDatabase".into()))
}

async fn await_request(req: &IdbRequest) -> Result<(), WebVaultErr> {
    let _ = await_request_with_result(req).await?;
    Ok(())
}

async fn await_request_with_result(req: &IdbRequest) -> Result<JsValue, WebVaultErr> {
    let req_clone = req.clone();
    let promise = Promise::new(&mut |resolve, reject| {
        let req_inner = req_clone.clone();
        let resolve_clone = resolve.clone();
        let onsuccess: Closure<dyn FnMut(JsValue)> = Closure::new(move |_evt: JsValue| {
            let v = req_inner.result().unwrap_or(JsValue::NULL);
            let _ = resolve_clone.call1(&JsValue::NULL, &v);
        });
        req_clone.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
        onsuccess.forget();

        let onerror: Closure<dyn FnMut(JsValue)> = Closure::new(move |_evt: JsValue| {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("idb request error"));
        });
        req_clone.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
    });
    JsFuture::from(promise)
        .await
        .map_err(|e| WebVaultErr::Tx(format_js(&e)))
}

fn format_js(v: &JsValue) -> String {
    if let Some(s) = v.as_string() {
        s
    } else {
        format!("{v:?}")
    }
}

