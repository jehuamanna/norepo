//! Test fixtures shared between this crate's tests and downstream crates.

use crate::error::StoreError;
use crate::sqlite::Store;

/// Returns a fully-migrated `:memory:` store. Each call yields a fresh DB.
pub fn fresh_store() -> Result<Store, StoreError> {
    Store::for_test()
}

/// Alias of [`fresh_store`] under the name used by Phase-1 plan / docs.
pub fn open_in_memory() -> Result<Store, StoreError> {
    fresh_store()
}
