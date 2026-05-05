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
            class: "operon-modal-scrim",
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
                class: "operon-modal-card",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title", "{title}" }
                p { class: "operon-modal-message", "{message}" }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        "data-testid": "confirm-dialog-cancel",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-danger",
                        "data-testid": "confirm-dialog-confirm",
                        onclick: move |_| on_confirm.call(()),
                        "{confirm_label}"
                    }
                }
            }
        }
    }
}
