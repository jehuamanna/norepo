//! Persistent data substrate for Operon-dioxus RBAC/ODU/TPN.
//!
//! Hosts the SQL schema, migration runner, repository traits, and SQLite
//! implementations. Local mode and the non-local server share this crate; the
//! only difference is the connection string passed to `Store::open`.

pub mod error;
pub mod ids;
pub mod migrations;
pub mod repos;
pub mod sqlite;
pub mod test_support;
pub mod time;
pub mod vfs;

// Plans-Phase-2-saving / Option 2: full SQLite-on-wasm.
// Activated by `--features wasm-sqlite` on a wasm32 target.
// Build prerequisite: `clang` on the host (sqlite-wasm-rs's build.rs
// compiles libsqlite3 C source for wasm32-unknown-unknown).
#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub mod wasm;

// Plans-Phase-2-saving / Option 2: cfg-gated re-export so repos can write
// `use crate::sql::{Connection, OptionalExtension};` and have it resolve
// to either rusqlite (desktop) or the wasm shim. The `params!` macro
// goes through `crate::params!` which is `#[macro_export]`-ed at the
// crate root by both rusqlite and the shim.
#[cfg(not(all(target_arch = "wasm32", feature = "wasm-sqlite")))]
pub mod sql {
    pub use rusqlite::ffi;
    pub use rusqlite::types;
    pub use rusqlite::{
        Connection, Error, OptionalExtension, Result, Row, Statement, Transaction,
    };
    /// Re-export rusqlite's `params!` macro at `crate::sql::params!` so
    /// repos have a single import path on both targets.
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

pub use error::StoreError;
pub use ids::*;
pub use sqlite::{Store, StoreConfig, StoreMode};
