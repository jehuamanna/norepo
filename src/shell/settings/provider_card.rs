//! Single provider's settings card (Slice A4b).
//!
//! Header (label + status badge) over a body that toggles between a masked
//! key view and an inline input. Buttons: Update / Verify / Remove.
//! All I/O routes through `SettingsService`; this file is purely UI.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use super::service::{ProviderId, SettingsService, VerifyOutcome};
use super::SettingsServiceCtx;

#[derive(Clone, Debug, PartialEq)]
enum CardStatus {
    Loading,
    Configured,
    NotConfigured,
    VerifyFailed(String),
    Verifying,
    VerifyOk,
}

#[derive(Clone, Debug, PartialEq, Props)]
pub struct ProviderCardProps {
    pub provider: ProviderId,
}

#[component]
pub fn ProviderCard(props: ProviderCardProps) -> Element {
    let provider = props.provider;
    let service: SettingsService = match try_consume_context::<SettingsServiceCtx>() {
        Some(SettingsServiceCtx(s)) => s,
        None => {
            return rsx! { div { class: "operon-settings-provider-card", "settings unavailable" } }
        }
    };

    let mut masked = use_signal(|| Option::<String>::None);
    let mut status = use_signal(|| CardStatus::Loading);
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);

    {
        let service = service.clone();
        use_effect(move || {
            let service = service.clone();
            spawn(async move {
                let m = service.masked_key(provider).await;
                let st = if m.is_some() {
                    CardStatus::Configured
                } else {
                    CardStatus::NotConfigured
                };
                masked.set(m);
                status.set(st);
            });
        });
    }

    let on_update_click = {
        let masked_now = masked.read().clone();
        move |_| {
            draft.set(String::new());
            error.set(None);
            editing.set(true);
            // Suppress masked while editing.
            let _ = masked_now;
        }
    };

    let on_save = {
        let service = service.clone();
        move |_| {
            let value = draft.read().trim().to_string();
            if value.is_empty() {
                error.set(Some("Key cannot be empty.".into()));
                return;
            }
            let service = service.clone();
            spawn(async move {
                match service.set_key(provider, &value).await {
                    Ok(()) => {
                        let m = service.masked_key(provider).await;
                        masked.set(m);
                        editing.set(false);
                        error.set(None);
                        status.set(CardStatus::Verifying);
                        match service.verify(provider).await {
                            VerifyOutcome::Ok => status.set(CardStatus::VerifyOk),
                            VerifyOutcome::Failed(e) => {
                                status.set(CardStatus::VerifyFailed(e))
                            }
                        }
                    }
                    Err(e) => error.set(Some(format!("save failed: {e}"))),
                }
            });
        }
    };

    let on_cancel = move |_| {
        editing.set(false);
        error.set(None);
    };

    let on_verify_click = {
        let service = service.clone();
        move |_| {
            let service = service.clone();
            status.set(CardStatus::Verifying);
            spawn(async move {
                match service.verify(provider).await {
                    VerifyOutcome::Ok => status.set(CardStatus::VerifyOk),
                    VerifyOutcome::Failed(e) => status.set(CardStatus::VerifyFailed(e)),
                }
            });
        }
    };

    let on_remove_click = {
        let service = service.clone();
        move |_| {
            let service = service.clone();
            spawn(async move {
                if service.remove_key(provider).await.is_ok() {
                    masked.set(None);
                    status.set(CardStatus::NotConfigured);
                }
            });
        }
    };

    let badge = match &*status.read() {
        CardStatus::Loading => ("loading…", "operon-settings-badge-neutral"),
        CardStatus::Configured => ("configured", "operon-settings-badge-ok"),
        CardStatus::NotConfigured => ("not configured", "operon-settings-badge-warn"),
        CardStatus::VerifyFailed(_) => ("verify failed", "operon-settings-badge-err"),
        CardStatus::Verifying => ("verifying…", "operon-settings-badge-neutral"),
        CardStatus::VerifyOk => ("verified", "operon-settings-badge-ok"),
    };
    let detail = match &*status.read() {
        CardStatus::VerifyFailed(msg) => Some(msg.clone()),
        _ => None,
    };
    let masked_label = masked.read().clone();
    let is_editing = *editing.read();
    let err_msg = error.read().clone();

    rsx! {
        div {
            class: "operon-settings-provider-card",
            "data-testid": "provider-card",
            "data-provider": "{provider.label()}",
            div { class: "operon-settings-provider-header",
                span { class: "operon-settings-provider-name", "{provider.label()}" }
                span { class: "operon-settings-badge {badge.1}", "{badge.0}" }
            }
            if let Some(d) = detail {
                p { class: "operon-settings-provider-detail", "{d}" }
            }
            if is_editing {
                div { class: "operon-settings-provider-edit",
                    input {
                        r#type: "password",
                        class: "operon-modal-input",
                        autofocus: true,
                        placeholder: "paste API key",
                        value: "{draft.read()}",
                        oninput: move |e| draft.set(e.value()),
                    }
                    if let Some(msg) = err_msg {
                        p { class: "operon-modal-error", "{msg}" }
                    }
                    div { class: "operon-settings-provider-actions",
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            onclick: on_cancel,
                            "Cancel"
                        }
                        button {
                            r#type: "button",
                            class: "operon-modal-button operon-modal-button-primary",
                            onclick: on_save,
                            "Save"
                        }
                    }
                }
            } else {
                div { class: "operon-settings-provider-body",
                    code { class: "operon-settings-key-mask",
                        {masked_label.unwrap_or_else(|| "—".to_string())}
                    }
                    div { class: "operon-settings-provider-actions",
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            onclick: on_update_click,
                            "Update"
                        }
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            onclick: on_verify_click,
                            "Verify"
                        }
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            onclick: on_remove_click,
                            "Remove"
                        }
                    }
                }
            }
        }
    }
}
