//! Local-Mode entry point.
//!
//! Phase-1 lands the startup chooser, a thin Local-Mode shell, and a
//! renameable "Local user" identity backed by the `local_user` /
//! `local_app_settings` SQLite tables. The cloud RBAG path stays untouched —
//! `app.rs` mounts either `LocalShell` or `Shell` based on `AppState.mode`.

#[cfg(not(target_arch = "wasm32"))]
pub mod desktop;
// In-tree MCP bridge: lets the PTY-hosted `claude` (and, in M4b.5,
// chat-mode Claude) call back into the GUI process. Unix-only — uses
// unix-domain sockets. The module is `#![cfg(all(unix, ...))]` so on
// Windows the symbol simply isn't there and the consuming code is
// `#[cfg]`-gated to match.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub mod bridge_runtime;

/// Cross-target wasm/Windows no-op so `App` can call
/// `provide_bridge_runtime()` unconditionally. The real
/// implementation lives in [`desktop::provide_bridge_runtime`] and
/// is re-exported through `pub use desktop::*` above on unix-desktop
/// targets; this stub fills in everywhere else.
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
pub fn provide_bridge_runtime() {}
#[cfg(not(target_arch = "wasm32"))]
pub mod editor;
#[cfg(not(target_arch = "wasm32"))]
pub mod explorer;
// Plans-Phase-2-saving / Phase E: ui/ is pure Dioxus + uuid, no
// platform-specific deps, so it compiles wherever we need it.
#[cfg(any(not(target_arch = "wasm32"), feature = "wasm-sqlite"))]
pub mod ui;
#[cfg(not(target_arch = "wasm32"))]
pub mod images;
#[cfg(not(target_arch = "wasm32"))]
pub mod note_lookup;
#[cfg(not(target_arch = "wasm32"))]
pub mod reload_socket;
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

// Wasm without wasm-sqlite: stub Local Mode (renders "unavailable" placeholders).
#[cfg(all(target_arch = "wasm32", not(feature = "wasm-sqlite")))]
mod wasm_stub;
#[cfg(all(target_arch = "wasm32", not(feature = "wasm-sqlite")))]
pub use wasm_stub::*;

// Plans-Phase-2-saving / Phase E: real wasm Local Mode shell, backed by
// the wasm Store + OpfsPersistence stack. Mounted only when the
// `wasm-sqlite` feature is on.
#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub mod wasm_init;
#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub mod wasm_shell;
#[cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]
pub use wasm_shell::*;

/// IndexedDB-backed persistence for the user's chosen OPFS handle (web only).
/// Phase 2 wires this into the web boot flow; Phase 1 ships the helpers so
/// they can be unit-tested ahead of the consumer landing.
#[cfg(target_arch = "wasm32")]
pub mod web_vault_handle;

/// Settings key used by [`StartupChooser`] to remember the last picked mode.
pub const SETTINGS_KEY_MODE_REMEMBERED: &str = "mode_remembered";
pub const MODE_VALUE_LOCAL: &str = "Local";
pub const MODE_VALUE_CLOUD: &str = "Cloud";

/// App-scope reactive flag for "user has picked a mode".
/// Initialised from the boot value of `mode_remembered`; flipped by
/// [`StartupChooser`] when the user clicks Local or Cloud so the App rsx
/// transitions out of the chooser without a restart.
#[derive(Clone, Copy)]
pub struct ModeChosen(pub dioxus::prelude::Signal<bool>);

/// Settings key holding the absolute path to the user's notes vault directory.
/// Set on first run by the `VaultDirPicker` modal; read at boot to decide
/// whether to mount the workspace or render the picker. Used as the root for
/// markdown bodies (`<vault>/notes/<id>.md`) and image blobs
/// (`<vault>/.operon/images/<sha>.<ext>`).
pub const SETTINGS_KEY_VAULT_ROOT: &str = "vault.root.path";

/// Companion-pane Claude model picker: last-chosen global default. Stored
/// value is the model slug (e.g. `claude-opus-4-7`); empty string means
/// "Default" (no `--model` override).
pub const SETTINGS_KEY_CLAUDE_DEFAULT_MODEL: &str = "claude.default_model";

/// Companion-pane `--permission-mode` picker: last-chosen global default.
/// Stored value is the raw CLI mode (`acceptEdits` | `plan` |
/// `bypassPermissions`); empty string means "(default)".
pub const SETTINGS_KEY_CLAUDE_DEFAULT_PERMISSION_MODE: &str =
    "claude.default_permission_mode";

/// Companion-pane surface selection: `chat` (default — the rich Operon
/// chat UI driven by `ClaudeCodeChatPlugin`) vs `claude_code` (a raw
/// PTY-backed terminal that runs the `claude` CLI directly so users
/// see the upstream TUI). Read by `CompanionArea` to decide which
/// component to mount; written by the global Settings panel toggle.
pub const SETTINGS_KEY_COMPANION_MODE: &str = "companion.mode";
pub const COMPANION_MODE_CHAT: &str = "chat";
pub const COMPANION_MODE_CLAUDE_CODE: &str = "claude_code";
