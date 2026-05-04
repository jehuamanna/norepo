//! Status bar: bottom-spanning row.
//!
//! Hosts the temporary theme-toggle button and a Phase-3-only debug button that opens a
//! hardcoded sample tab (so the developer can smoke-test [`crate::tabs::TabManager`]).
//! The debug button is removed in Phase 4 once `NotesExplorer` becomes the entry point.

use dioxus::prelude::*;

use crate::plugin::manifest::NoteKind;
use crate::tabs::TabManager;
use crate::theme::{self, Theme, ThemeMode};

#[component]
pub fn StatusBar() -> Element {
    let mut theme_signal: Signal<Theme> = use_context();
    let mut tabs: Signal<TabManager> = use_context();

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
                "data-action": "open-sample",
                style: "background: transparent; color: inherit; border: 1px solid var(--vscode-panel-border); padding: 2px 8px; cursor: pointer; font: inherit; margin-right: 8px;",
                onclick: move |_| {
                    tabs.write().open(
                        "dev/sample".into(),
                        NoteKind::Markdown,
                        "Sample".into(),
                        "hello".into(),
                    );
                },
                "Open Sample"
            }
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
