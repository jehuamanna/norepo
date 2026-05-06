//! Local-Mode entry point.
//!
//! Phase-1 lands the startup chooser, a thin Local-Mode shell, and a
//! renameable "Local user" identity backed by the `local_user` /
//! `local_app_settings` SQLite tables. The cloud RBAG path stays untouched —
//! `app.rs` mounts either `LocalShell` or `Shell` based on `AppState.mode`.

#[cfg(not(target_arch = "wasm32"))]
pub mod desktop;
#[cfg(not(target_arch = "wasm32"))]
pub mod editor;
#[cfg(not(target_arch = "wasm32"))]
pub mod explorer;
#[cfg(not(target_arch = "wasm32"))]
pub mod ui;
#[cfg(not(target_arch = "wasm32"))]
pub mod images;
#[cfg(not(target_arch = "wasm32"))]
pub mod vault;
#[cfg(not(target_arch = "wasm32"))]
pub mod vault_picker;
#[cfg(not(target_arch = "wasm32"))]
pub use vault_picker::VaultDirPicker;
#[cfg(not(target_arch = "wasm32"))]
pub use desktop::*;
#[cfg(not(target_arch = "wasm32"))]
pub use editor::{LocalNoteEditor, LocalSaveAction, LocalSaveButton};
#[cfg(not(target_arch = "wasm32"))]
pub use explorer::{ExplorerPanel, LocalProjectVersion, SelectedNote, SelectedProject};

#[cfg(target_arch = "wasm32")]
mod wasm_stub;
#[cfg(target_arch = "wasm32")]
pub use wasm_stub::*;

/// IndexedDB-backed persistence for the user's chosen OPFS handle (web only).
/// Phase 2 wires this into the web boot flow; Phase 1 ships the helpers so
/// they can be unit-tested ahead of the consumer landing.
#[cfg(target_arch = "wasm32")]
pub mod web_vault_handle;

/// Settings key used by [`StartupChooser`] to remember the last picked mode.
pub const SETTINGS_KEY_MODE_REMEMBERED: &str = "mode_remembered";
pub const MODE_VALUE_LOCAL: &str = "Local";
pub const MODE_VALUE_CLOUD: &str = "Cloud";

/// Settings key holding the absolute path to the user's notes vault directory.
/// Set on first run by the `VaultDirPicker` modal; read at boot to decide
/// whether to mount the workspace or render the picker. Used as the root for
/// markdown bodies (`<vault>/notes/<id>.md`) and image blobs
/// (`<vault>/.operon/images/<sha>.<ext>`).
pub const SETTINGS_KEY_VAULT_ROOT: &str = "vault.root.path";
