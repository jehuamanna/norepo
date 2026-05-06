//! Plans-Phase-2-saving: wasm Store wrapper that mirrors the desktop
//! `sqlite::Store` API surface.
//!
//! Backed by `sqlite-wasm-rs` FFI. By default the database lives in the
//! main-memory VFS (no persistence); when callers register the
//! `opfs-sahpool` VFS first (companion crate `sqlite-wasm-vfs`) and pass
//! a URI of the form `file:operon.sqlite?vfs=opfs-sahpool`, the data
//! survives reload via OPFS Sync Access Handles.
//!
//! `Store::conn()` returns a `WasmConnGuard` that derefs to
//! [`crate::wasm::sql::Connection`]. The guard holds a Mutex lock over a
//! single shared connection (sqlite-wasm-rs is `SQLITE_THREADSAFE=0`,
//! main-thread-only — the Mutex makes mis-use detectable rather than UB).

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

use core::ffi::c_int;
use std::ffi::CString;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};

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
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Wasm Store. Cloning this is cheap — the underlying `Connection` is
/// shared via `Arc<Mutex<_>>`. Any number of `Store` clones can exist at
/// once; only one `WasmConnGuard` (the result of `conn()`) is live at a
/// time across the entire crate.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open a Store at `filename`. Pass `:memory:` for ephemeral storage,
    /// or `file:operon.sqlite?vfs=opfs-sahpool` once the OPFS VFS is
    /// registered. Applies the same pragmas as the desktop store
    /// (foreign_keys=ON, synchronous=NORMAL); WAL is skipped because
    /// neither the in-memory nor opfs-sahpool VFS supports it.
    pub fn open(filename: &str) -> Result<Self, StoreError> {
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
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StoreError::Open(format!("pragma fk: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| StoreError::Open(format!("pragma sync: {e}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::open(":memory:")
    }

    /// Acquire the shared connection. Returns a `WasmConnGuard` whose
    /// `Deref<Target = Connection>` impl makes existing repo code
    /// (`let conn = self.store.conn()?; conn.prepare(...);`) compile
    /// unchanged.
    pub fn conn(&self) -> Result<WasmConnGuard<'_>, StoreError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| StoreError::Open("connection mutex poisoned".into()))?;
        Ok(WasmConnGuard { guard })
    }
}

/// Mutex-guarded shared connection handle. Mirrors the
/// `r2d2::PooledConnection<SqliteConnectionManager>` shape just enough
/// for the existing repo call sites: `Deref<Target = Connection>` and
/// `DerefMut`.
pub struct WasmConnGuard<'a> {
    guard: MutexGuard<'a, Connection>,
}

impl<'a> Deref for WasmConnGuard<'a> {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &*self.guard
    }
}

impl<'a> DerefMut for WasmConnGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.guard
    }
}
