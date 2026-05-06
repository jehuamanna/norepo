//! wasm32 stub for the Local Mode entry point.
//!
//! The browser build does not link `operon-store` (rusqlite is bundled C code
//! that doesn't target wasm). The cloud RBAG path is the only mode currently
//! available on web — the chooser and shell render disabled placeholders so
//! the build still links cleanly and `app.rs` can mount the module
//! unconditionally.

use dioxus::prelude::*;

use crate::rbag::state::Mode;
use crate::tabs::TabId;

/// Stub of the desktop-only LocalSaveAction. The web build never reaches a
/// Local-Mode tab (rusqlite isn't linked), so the callback is a no-op kept
/// only so that try_consume_context::<LocalSaveAction>() in shell code can
/// still type-check on wasm.
#[derive(Clone, PartialEq)]
pub struct LocalSaveAction {
    pub callback: Callback<()>,
}

/// Stub: gear → settings panel signal.
#[derive(Clone, Copy)]
pub struct SettingsOpen(pub Signal<bool>);

/// Stub: latest Local username.
#[derive(Clone, Copy)]
pub struct LocalUsername(pub Signal<String>);

#[component]
pub fn LocalShellOverlay(children: Element) -> Element {
    rsx! { {children} }
}

#[component]
pub fn ExplorerPanel() -> Element {
    rsx! {
        div {
            class: "operon-local-editor-empty",
            "Local Mode unavailable on the web build."
        }
    }
}

#[component]
pub fn LocalNoteEditor(tab_id: TabId, action: LocalSaveAction) -> Element {
    let _ = (tab_id, action);
    rsx! { div { "Local Mode unavailable on the web build." } }
}

pub fn provide_local_app_signals() {}

#[component]
pub fn StartupChooser() -> Element {
    rsx! {
        div {
            class: "flex items-center justify-center h-screen w-screen text-sm",
            "data-testid": "mode-chooser",
            "Local mode unavailable on web; cloud mode is selected automatically."
        }
    }
}

#[component]
pub fn LocalStateProvider(children: Element) -> Element {
    rsx! { {children} }
}

pub fn provide_local_state() {}

pub fn read_remembered_mode_web() -> Option<Mode> {
    Some(Mode::NonLocal)
}
