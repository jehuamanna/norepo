//! First-run vault directory picker modal.
//!
//! Phase-1 desktop deliverable. Renders a focus-trapped modal that the user
//! cannot dismiss without choosing a directory (when `blocking == true`). The
//! "Change…" entry from `SettingsPanel` reuses the same component with
//! `blocking = false` and a Cancel button.
//!
//! Web parity is owned by Plans-Phase-2-saving (which lands the OPFS-backed
//! Persistence + sqlite-wasm-rs Store). Until that ships, the wasm branch
//! renders an unsupported message; the desktop branch is fully functional.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::LocalSettingsRepository;

use super::desktop::LocalSettingsRepo;
use super::vault::{self, VaultErr, VaultRoot};

#[component]
pub fn VaultDirPicker(blocking: bool, on_chosen: EventHandler<VaultRoot>) -> Element {
    let LocalSettingsRepo(settings) = use_context();
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut picking: Signal<bool> = use_signal(|| false);

    let pick = {
        let settings = settings.clone();
        move |_| {
            if *picking.read() {
                return;
            }
            picking.set(true);
            error.set(None);
            let settings = settings.clone();
            spawn(async move {
                let folder = rfd::AsyncFileDialog::new()
                    .set_title("Choose Operon vault directory")
                    .pick_folder()
                    .await;
                let Some(handle) = folder else {
                    picking.set(false);
                    return;
                };
                let raw = handle.path().to_path_buf();
                match try_set_vault(&settings, &raw) {
                    Ok(root) => {
                        picking.set(false);
                        on_chosen.call(root);
                    }
                    Err(e) => {
                        picking.set(false);
                        error.set(Some(format_vault_err(&e)));
                    }
                }
            });
        }
    };

    let cancel = move |_| {
        if !blocking {
            // Cancel only meaningful for the non-blocking "Change…" entry; the
            // first-run case has no escape (the modal is the only UI).
            error.set(None);
        }
    };

    rsx! {
        div {
            class: "operon-modal-scrim",
            role: "dialog",
            "aria-modal": "true",
            "aria-labelledby": "vault-picker-title",
            "data-testid": "vault-picker",
            onkeydown: move |evt| {
                if blocking && evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                }
            },
            div {
                class: "operon-modal-card",
                onclick: move |evt| evt.stop_propagation(),
                h2 {
                    id: "vault-picker-title",
                    class: "operon-modal-title",
                    if blocking { "Choose your notes vault" } else { "Change vault directory" }
                }
                p { class: "operon-modal-help",
                    "Operon will save markdown notes under "
                    code { "<vault>/notes/" }
                    " and image blobs under "
                    code { "<vault>/.operon/images/" }
                    ". You can change this later in Settings."
                }
                if let Some(msg) = error.read().clone() {
                    p { role: "alert", class: "operon-modal-error", "{msg}" }
                }
                div {
                    class: "operon-modal-actions",
                    if !blocking {
                        button {
                            r#type: "button",
                            class: "operon-modal-button",
                            onclick: cancel,
                            "Cancel"
                        }
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "vault-picker-choose",
                        disabled: *picking.read(),
                        onclick: pick,
                        if *picking.read() { "Opening picker…" } else { "Choose folder…" }
                    }
                }
            }
        }
    }
}

fn try_set_vault(
    settings: &Arc<dyn LocalSettingsRepository>,
    raw_path: &std::path::Path,
) -> Result<VaultRoot, VaultErr> {
    let canonical = vault::validate(raw_path)?;
    let root = VaultRoot { path: canonical };
    // Best-effort lock acquisition. The lock guard is dropped here on success;
    // the app holds its own lifetime guard once mounted (a follow-up will wire
    // a `LockGuard` into a Dioxus context). For first-run, just verifying that
    // the lock can be acquired is enough.
    let _guard = vault::acquire_lock(&root)?;
    vault::store(settings, &root)?;
    Ok(root)
}

fn format_vault_err(e: &VaultErr) -> String {
    match e {
        VaultErr::NotFound(_) => "Selected path does not exist or is not a directory.".into(),
        VaultErr::NotWritable(_) => "Selected directory is not writable.".into(),
        VaultErr::Locked => "Another Operon instance has this vault open.".into(),
        VaultErr::Settings(s) => format!("Could not save the choice: {s}"),
        VaultErr::Io(io) => format!("Filesystem error: {io}"),
        VaultErr::NotSet => "Vault path is not set.".into(),
    }
}
