//! Plans-Phase-2-saving / Option 2: full SQLite-on-wasm.
//!
//! Activated by `--features wasm-sqlite` on the wasm32 target. The module
//! ships a thin rusqlite-compat shim (`sql`) over the `sqlite-wasm-rs` FFI
//! plus a `Store` that mirrors the desktop `sqlite::Store` API surface so
//! repos compile against the same identifiers.
//!
//! Runtime requirement: `clang` on the host (sqlite-wasm-rs's build.rs
//! compiles bundled libsqlite3 C source for wasm32-unknown-unknown).
//! Document this in operon-store/Cargo.toml's feature description.

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

pub mod sql;
pub mod store;

pub use sql::{
    params_slice, Connection, Error, FromSql, OptionalExtension, Result, Row, Statement, ToSql,
    Transaction, Type,
};
pub use store::Store;
