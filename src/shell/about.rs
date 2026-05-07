//! About dialog — surfaced from `Help → About` and the command palette.
//!
//! Visibility lives in an `AboutOpen(Signal<bool>)` provided at App scope so
//! the `help.about` command (in `commands::builtins`) can flip it without
//! prop-drilling. The dialog reuses the `operon-modal-*` CSS classes that
//! `ConfirmDialog` already styles, so no new CSS is required.

use dioxus::prelude::*;

/// App-scope visibility signal for the About dialog. Provided in `App` and
/// flipped by `help.about`. The dialog component owns the `false` write
/// when the user dismisses it (Esc / scrim click / Close button).
#[derive(Clone, Copy)]
pub struct AboutOpen(pub Signal<bool>);

#[component]
pub fn AboutDialog() -> Element {
    let AboutOpen(mut open) = use_context();
    if !*open.read() {
        return rsx! {};
    }

    // Pull the binary's compile-time version straight from Cargo so the
    // dialog stays accurate without manual edits.
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    let close = move |_| open.set(false);

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "about-dialog",
            onclick: close,
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    open.set(false);
                }
            },
            tabindex: "0",
            div {
                class: "operon-modal-card",
                style: "max-width: 460px;",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title", "About Operon" }
                p {
                    class: "operon-modal-message",
                    "Operon is a local-first note workspace built with Dioxus and Rust. It pairs a VS Code-style chrome with a SQLite-backed vault so projects, notes, and their nested structure stay on your disk and under your control."
                }
                p {
                    class: "operon-modal-message",
                    style: "margin-top: 6px;",
                    "Version: {VERSION}"
                }
                p {
                    class: "operon-modal-message",
                    style: "margin-top: 12px;",
                    "Author: Jehu Shalom Amanna"
                }
                p {
                    class: "operon-modal-message",
                    "Email: "
                    a {
                        href: "mailto:jehuamanna@gmail.com",
                        style: "color: var(--operon-accent, #0078D4);",
                        "jehuamanna@gmail.com"
                    }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "about-dialog-close",
                        onclick: close,
                        "Close"
                    }
                }
            }
        }
    }
}
