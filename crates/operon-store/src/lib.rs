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

pub use error::StoreError;
pub use ids::*;
pub use sqlite::{Store, StoreConfig, StoreMode};
