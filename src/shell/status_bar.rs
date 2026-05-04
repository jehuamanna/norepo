//! Status bar: bottom-spanning row.
//!
//! Hosts a temporary theme-toggle button that flips the context-provided
//! [`crate::theme::ThemeSignal`] between light and dark.

use dioxus::prelude::*;

use crate::theme::{self, Theme, ThemeMode};

#[component]
pub fn StatusBar() -> Element {
    let mut theme_signal: Signal<Theme> = use_context();

    let mode_label = match theme_signal.read().mode {
        ThemeMode::Dark => "Dark",
        ThemeMode::Light => "Light",
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
                    let next = match theme_signal.read().mode {
                        ThemeMode::Dark => theme::defaults::light(),
                        ThemeMode::Light => theme::defaults::dark(),
                    };
                    theme_signal.set(next);
                },
                "Theme: {mode_label}"
            }
        }
    }
}
