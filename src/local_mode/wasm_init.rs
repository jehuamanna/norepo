//! Plans-Phase-2-saving / Phase E: async boot helper for wasm Local Mode.
//!
//! The desktop Local-Mode shell opens `Store::open(StoreConfig::local(path))`
//! synchronously at app boot. Wasm needs an async path because:
//! - OPFS file handles are returned by Promise-based APIs.
//! - The opfs-sahpool VFS install is async.
//!
//! [`init_wasm_local_mode`] installs the VFS, opens the wasm Store, runs
//! migrations, and resolves an `OpfsPersistence` rooted at the user's
//! vault `notes/` subdirectory. Returns the (Store, Persistence) pair so
//! the shell can stash both in Dioxus contexts.
//!
//! Activated only with `--features wasm-sqlite`. Without the feature, the
//! existing `wasm_stub::MemoryPersistence` continues to apply and this
//! module isn't compiled.

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

use std::sync::Arc;

use operon_store::{Store, StoreError};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{FileSystemDirectoryHandle, FileSystemGetDirectoryOptions};

use crate::persistence::{OpfsPersistence, Persistence};

/// Outcome of wasm Local Mode init: SQLite metadata Store + OPFS-backed
/// note-body Persistence. Both are wired through Dioxus context.
pub struct WasmLocalInit {
    pub store: Store,
    pub persistence: Arc<dyn Persistence>,
}

/// Open the wasm Local Mode plumbing.
///
/// Steps:
/// 1. Install the `opfs-sahpool` VFS (idempotent; safe across reloads).
/// 2. Resolve a `notes/` subdirectory under the user's chosen vault
///    handle (creating it if missing) — that's the file root for
///    [`OpfsPersistence`].
/// 3. Open `Store::open("file:operon.sqlite?vfs=opfs-sahpool")`. This
///    auto-creates the SQLite file in OPFS on first launch.
/// 4. Run migrations.
///
/// Returns a [`WasmLocalInit`] the shell installs as Dioxus context.
pub async fn init_wasm_local_mode(
    vault_handle: &FileSystemDirectoryHandle,
) -> Result<WasmLocalInit, StoreError> {
    // Step 1: register the VFS. The shipping default OPFS-SAH directory
    // name is `.opfs-sahpool`; we keep that because the VFS implementation
    // assumes exclusive ownership of that subdir.
    Store::install_opfs_vfs(".opfs-sahpool").await?;

    // Step 2: get/create `<vault>/notes/`.
    let opts = FileSystemGetDirectoryOptions::new();
    opts.set_create(true);
    let notes_promise = vault_handle.get_directory_handle_with_options("notes", &opts);
    let notes_value = JsFuture::from(notes_promise)
        .await
        .map_err(|e| StoreError::Open(format!("get notes/ handle: {e:?}")))?;
    let notes_dir: FileSystemDirectoryHandle = notes_value
        .dyn_into()
        .map_err(|v| StoreError::Open(format!("notes/ not a directory handle: {v:?}")))?;

    // Step 3: open the SQLite metadata DB on OPFS.
    let store = Store::open("file:operon.sqlite?vfs=opfs-sahpool")?;

    // Step 4: run migrations.
    store.with_conn_mut(|conn| {
        operon_store::migrations::migrate_up(conn)
    })?;

    let persistence: Arc<dyn Persistence> = Arc::new(OpfsPersistence::new(notes_dir));
    Ok(WasmLocalInit { store, persistence })
}
