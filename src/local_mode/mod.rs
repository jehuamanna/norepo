//! Local-Mode entry point.
//!
//! Phase-1 lands the startup chooser, a thin Local-Mode shell, and a
//! renameable "Local user" identity backed by the `local_user` /
//! `local_app_settings` SQLite tables. The cloud RBAG path stays untouched —
//! `app.rs` mounts either `LocalShell` or `Shell` based on `AppState.mode`.

#[cfg(not(target_arch = "wasm32"))]
pub mod desktop;
#[cfg(not(target_arch = "wasm32"))]
pub mod explorer;
#[cfg(not(target_arch = "wasm32"))]
pub mod ui;
#[cfg(not(target_arch = "wasm32"))]
pub use desktop::*;
#[cfg(not(target_arch = "wasm32"))]
pub use explorer::{ExplorerPanel, LocalProjectVersion, SelectedProject};

#[cfg(target_arch = "wasm32")]
mod wasm_stub;
#[cfg(target_arch = "wasm32")]
pub use wasm_stub::*;

/// Settings key used by [`StartupChooser`] to remember the last picked mode.
pub const SETTINGS_KEY_MODE_REMEMBERED: &str = "mode_remembered";
pub const MODE_VALUE_LOCAL: &str = "Local";
pub const MODE_VALUE_CLOUD: &str = "Cloud";
