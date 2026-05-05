//! Lightweight modal confirmation dialog. Used by destructive actions
//! (project delete, later: note delete) that need an explicit acknowledgement.

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct ConfirmDialogProps {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub on_confirm: Callback<()>,
    pub on_cancel: Callback<()>,
}

#[component]
pub fn ConfirmDialog(props: ConfirmDialogProps) -> Element {
    let on_confirm = props.on_confirm;
    let on_cancel = props.on_cancel;
    let title = props.title.clone();
    let message = props.message.clone();
    let confirm_label = props.confirm_label.clone();

    rsx! {
        div {
            class: "fixed inset-0 bg-black/40 flex items-center justify-center z-[60]",
            "data-testid": "confirm-dialog",
            onclick: move |_| on_cancel.call(()),
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                if key == "Escape" {
                    evt.prevent_default();
                    on_cancel.call(());
                } else if key == "Enter" {
                    evt.prevent_default();
                    on_confirm.call(());
                }
            },
            div {
                class: "bg-[var(--operon-bg)] text-[var(--operon-fg)] border border-[var(--operon-border)] rounded-md p-4 w-96 shadow-lg",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "text-sm font-semibold mb-2", "{title}" }
                p { class: "text-sm opacity-80 mb-4 whitespace-pre-line", "{message}" }
                div {
                    class: "flex justify-end gap-2",
                    button {
                        r#type: "button",
                        class: "px-3 py-1 text-xs rounded border border-[var(--operon-border)]",
                        "data-testid": "confirm-dialog-cancel",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "px-3 py-1 text-xs rounded bg-red-600 text-white",
                        "data-testid": "confirm-dialog-confirm",
                        onclick: move |_| on_confirm.call(()),
                        "{confirm_label}"
                    }
                }
            }
        }
    }
}
