//! Status bar: bottom-spanning row.
//!
//! Hosts the temporary theme-toggle button. The Phase-3 "Open Sample" debug button was
//! removed in Phase 4 once `NotesExplorer` became the entry point for opening notes.

use dioxus::prelude::*;

use crate::theme::{self, Theme, ThemeKind};

#[component]
pub fn StatusBar() -> Element {
    let mut theme_signal: Signal<Theme> = use_context();

    let mode_label = match theme_signal.read().kind {
        ThemeKind::Dark => "Dark",
        ThemeKind::Light => "Light",
        ThemeKind::HighContrast => "HC",
    };

    rsx! {
        section {
            "data-region": "status-bar",
            class: "operon-region operon-status-bar",
            span { class: "operon-status-bar-label", "Operon" }
            span { style: "flex: 1 1 auto;" }
            button {
                class: "operon-status-toggle",
                "data-action": "toggle-theme",
                style: "background: transparent; color: inherit; border: 1px solid var(--vscode-panel-border); padding: 2px 8px; cursor: pointer; font: inherit;",
                onclick: move |_| {
                    let next = match theme_signal.read().kind {
                        ThemeKind::Dark | ThemeKind::HighContrast => theme::defaults::light(),
                        ThemeKind::Light => theme::defaults::dark(),
                    };
                    theme_signal.set(next);
                },
                "Theme: {mode_label}"
            }
        }
    }
}
