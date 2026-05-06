//! Plans-Phase-2-saving: wasm Store wrapper that mirrors the desktop
//! `sqlite::Store` API surface (`open`, `conn`-style access).
//!
//! Backed by `sqlite-wasm-rs` FFI + (when available) the `opfs-sahpool`
//! VFS for OPFS-backed durability. The Store opens an in-memory database
//! by default; persistence lands when `sqlite-wasm-vfs` (companion crate)
//! is wired in.

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

use core::ffi::c_int;
use std::ffi::CString;
use std::ptr;
use std::sync::{Arc, Mutex};

use sqlite_wasm_rs as ffi;

use super::sql::{ffi_consts, Connection};
use crate::error::StoreError;

fn errmsg(db: *mut ffi::sqlite3) -> String {
    if db.is_null() {
        return "(null db)".into();
    }
    let p = unsafe { ffi::sqlite3_errmsg(db) };
    if p.is_null() {
        "(no message)".into()
    } else {
        unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// Wasm Store. Holds a single shared `Connection` guarded by a Mutex
/// (sqlite-wasm-rs is `SQLITE_THREADSAFE=0`; main-thread-only).
#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open a Store on the given OPFS file path. `filename` is interpreted
    /// by SQLite — for in-memory pass `:memory:`. For OPFS persistence,
    /// pass `"file:operon.sqlite?vfs=opfs-sahpool"` after the VFS has
    /// been registered via `sqlite-wasm-vfs::sahpool::install`.
    pub fn open(filename: &str) -> std::result::Result<Self, StoreError> {
        let mut db: *mut ffi::sqlite3 = ptr::null_mut();
        let cname = CString::new(filename)
            .map_err(|e| StoreError::Open(format!("invalid filename: {e}")))?;
        let flags = (ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | ffi::SQLITE_OPEN_URI)
            as c_int;
        let rc = unsafe {
            ffi::sqlite3_open_v2(cname.as_ptr(), &mut db as *mut _, flags, ptr::null())
        };
        if rc != ffi_consts::SQLITE_OK as c_int {
            let msg = errmsg(db);
            unsafe { ffi::sqlite3_close_v2(db) };
            return Err(StoreError::Open(format!("sqlite3_open_v2 rc={rc}: {msg}")));
        }
        let conn = Connection::from_raw(db);
        // Apply the same pragmas the desktop store uses.
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StoreError::Open(format!("pragma fk: {e}")))?;
        // WAL is unavailable on most wasm VFS implementations; opfs-sahpool
        // doesn't support it. Skip on wasm.
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| StoreError::Open(format!("pragma sync: {e}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> std::result::Result<Self, StoreError> {
        Self::open(":memory:")
    }

    /// Acquire the underlying `Connection` for the duration of a closure.
    /// The desktop API exposes `Store::conn() -> PooledConnection`; on
    /// wasm we lock the single connection and pass it as an argument.
    pub fn with_conn<R>(
        &self,
        f: impl FnOnce(&Connection) -> std::result::Result<R, StoreError>,
    ) -> std::result::Result<R, StoreError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| StoreError::Open("connection mutex poisoned".into()))?;
        f(&*guard)
    }

    pub fn with_conn_mut<R>(
        &self,
        f: impl FnOnce(&mut Connection) -> std::result::Result<R, StoreError>,
    ) -> std::result::Result<R, StoreError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| StoreError::Open("connection mutex poisoned".into()))?;
        f(&mut *guard)
    }
}
