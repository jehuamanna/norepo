//! Lightweight modal confirmation dialog. Used by destructive actions
//! (project delete, later: note delete) that need an explicit acknowledgement.

use std::rc::Rc;

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

    // Tracks which button currently has focus so Tab can swap to the other
    // one. 0 = Cancel (default on mount), 1 = Delete/confirm.
    let mut focused: Signal<u8> = use_signal(|| 0);
    let mut cancel_handle: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let mut delete_handle: Signal<Option<Rc<MountedData>>> = use_signal(|| None);

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
                }
            },
            div {
                class: "operon-modal-card",
                tabindex: "-1",
                onclick: move |evt| evt.stop_propagation(),
                onkeydown: move |evt| {
                    let key = evt.key().to_string();
                    if key == "Tab" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        let cur = *focused.peek();
                        let target = if cur == 0 {
                            delete_handle.peek().clone()
                        } else {
                            cancel_handle.peek().clone()
                        };
                        if let Some(h) = target {
                            spawn(async move {
                                let _ = h.set_focus(true).await;
                            });
                        }
                    }
                },
                h2 { class: "operon-modal-title", "{title}" }
                p { class: "operon-modal-message", "{message}" }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        "data-testid": "confirm-dialog-cancel",
                        onmounted: move |evt| {
                            cancel_handle.set(Some(evt.data()));
                            drop(evt.set_focus(true));
                        },
                        onfocus: move |_| focused.set(0),
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-danger",
                        "data-testid": "confirm-dialog-confirm",
                        onmounted: move |evt| {
                            delete_handle.set(Some(evt.data()));
                        },
                        onfocus: move |_| focused.set(1),
                        onclick: move |_| on_confirm.call(()),
                        "{confirm_label}"
                    }
                }
            }
        }
    }
}
