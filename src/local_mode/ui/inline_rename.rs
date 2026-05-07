//! Inline rename input. Mounted inside a row to replace the label while the
//! user types. Enter or blur commits the trimmed value via `on_commit`;
//! Escape cancels without firing `on_commit`.

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct InlineRenameProps {
    pub initial: String,
    pub on_commit: Callback<String>,
    pub on_cancel: Callback<()>,
}

#[component]
pub fn InlineRename(props: InlineRenameProps) -> Element {
    let mut value: Signal<String> = use_signal(|| props.initial.clone());
    // Track whether we already fired on_commit/on_cancel so blur doesn't
    // double-fire after Enter or Escape.
    let mut settled: Signal<bool> = use_signal(|| false);

    let on_commit = props.on_commit;
    let on_cancel = props.on_cancel;

    rsx! {
        input {
            r#type: "text",
            class: "w-full px-1 py-0.5 text-sm bg-[var(--operon-input-bg)] border border-[var(--operon-border)] rounded outline-none",
            "data-testid": "inline-rename-input",
            value: "{value.read()}",
            autofocus: true,
            onmounted: move |evt| {
                // `set_focus` returns a Future; native (desktop) rendering resolves
                // synchronously, but clippy still flags the unawaited future. Drop
                // it explicitly so the lint is satisfied.
                drop(evt.set_focus(true));
                // Pre-select the input's text so a freshly-created note can be
                // retitled by typing without first deleting the placeholder.
                // Dioxus desktop hosts a webview, so <input>.select() is reachable
                // via the portable eval bridge on both web and desktop targets.
                // Only one inline-rename input is ever mounted at a time (gated
                // by the `renaming_note` signal), so a global selector is safe.
                let _ = dioxus::document::eval(
                    "const el = document.querySelector('[data-testid=\"inline-rename-input\"]'); if (el) el.select();"
                );
            },
            oninput: move |evt| value.set(evt.value()),
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                if key == "Enter" {
                    evt.prevent_default();
                    if !*settled.read() {
                        settled.set(true);
                        on_commit.call(value.read().clone());
                    }
                } else if key == "Escape" {
                    evt.prevent_default();
                    if !*settled.read() {
                        settled.set(true);
                        on_cancel.call(());
                    }
                }
            },
            onblur: move |_| {
                if !*settled.read() {
                    settled.set(true);
                    on_commit.call(value.read().clone());
                }
            },
            // Clicks on the input itself shouldn't propagate up to row handlers
            // (e.g. selection / context-menu).
            onclick: move |evt| evt.stop_propagation(),
        }
    }
}
