//! wasm32 stub for the Local Mode entry point.
//!
//! The browser build does not link `operon-store` (rusqlite is bundled C code
//! that doesn't target wasm). The cloud RBAG path is the only mode currently
//! available on web — the chooser and shell render disabled placeholders so
//! the build still links cleanly and `app.rs` can mount the module
//! unconditionally.

use dioxus::prelude::*;

use crate::rbag::state::Mode;

#[component]
pub fn LocalShellOverlay(children: Element) -> Element {
    rsx! { {children} }
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
