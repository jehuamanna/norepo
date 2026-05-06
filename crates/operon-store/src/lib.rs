//! Persistent data substrate for Operon-dioxus RBAC/ODU/TPN.
//!
//! Hosts the SQL schema, migration runner, repository traits, and SQLite
//! implementations. Local mode and the non-local server share this crate; the
//! only difference is the connection string passed to `Store::open`.

pub mod error;
pub mod ids;
pub mod time;

// Plans-Phase-2-saving: SQL-using modules are present when either the
// desktop rusqlite back-end or the wasm-sqlite shim is active. On wasm
// without the feature, operon-store still compiles but only exposes
// error/ids/time — useful as a typed surface for shared id helpers.
#[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
pub mod migrations;
#[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
pub mod repos;
#[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
pub mod vfs;

// Desktop-only (uses r2d2 + rusqlite directly).
#[cfg(not(target_arch = "wasm32"))]
pub mod sqlite;
#[cfg(not(target_arch = "wasm32"))]
pub mod test_support;

// Plans-Phase-2-saving / Option 2: full SQLite-on-wasm. Activated by
// `--features wasm-sqlite` on a wasm32 target. Build prerequisite:
// `clang` on the host.
#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub mod wasm;

// Plans-Phase-2-saving: cfg-gated re-export so repos can write
// `use crate::sql::{Connection, OptionalExtension};` and have it resolve
// to either rusqlite (desktop) or the wasm shim.
#[cfg(not(target_arch = "wasm32"))]
pub mod sql {
    pub use rusqlite::ffi;
    pub use rusqlite::types;
    pub use rusqlite::{
        Connection, Error, OptionalExtension, Result, Row, Statement, Transaction,
    };
    pub use rusqlite::params;
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub mod sql {
    pub use crate::wasm::sql::{
        Connection, Error, OptionalExtension, Result, Row, Statement, Transaction, Type,
    };
    pub use crate::wasm::sql::ffi_consts as ffi;
    pub mod types {
        pub use super::Type;
    }
    pub use crate::params;
}

/// Plans-Phase-2-saving: `Store` proxy module. Repos `use crate::store::Store;`
/// and get the right backend. Desktop = rusqlite-backed `sqlite::Store`
/// with r2d2 pooling; wasm-sqlite = `wasm::Store` with a Mutex-shared
/// connection.
#[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
pub mod store {
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::sqlite::Store;
    #[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
    pub use crate::wasm::Store;
}

pub use error::StoreError;
pub use ids::*;

#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::{Store, StoreConfig, StoreMode};

#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub use wasm::Store;
