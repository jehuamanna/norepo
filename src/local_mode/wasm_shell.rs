//! Plans-Phase-2-saving / Phase E: minimal wasm Local Mode shell.
//!
//! Activated by `--features wasm-sqlite` on a wasm32 target. Replaces the
//! `wasm_stub::*` placeholders with real components that talk to the
//! wasm `Store` + `OpfsPersistence` resolved via [`init_wasm_local_mode`].
//!
//! This is a v1 shell — it doesn't try to mirror every desktop affordance
//! (multi-select, drag-and-drop, image notes, wikilink picker etc.). It
//! demonstrates that the operon-store stack runs end-to-end on web:
//! pick a vault, see your projects + notes, open a note, edit, save.
//! The richer per-row UX from `local_mode/explorer/` and editor flows
//! lights up when their cfg gates are widened to wasm in a follow-up.

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::{
    LocalNoteRepository, LocalProjectRepository, LocalSettingsRepository,
    SqliteLocalNoteRepository, SqliteLocalProjectRepository, SqliteLocalSettingsRepository,
};
use operon_store::Store;
use uuid::Uuid;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::FileSystemDirectoryHandle;

use crate::persistence::Persistence;
use crate::rbag::state::Mode;
use crate::tabs::TabId;

use super::wasm_init;

/// App-scope context: handle to the user's chosen OPFS vault.
#[derive(Clone)]
pub struct WasmVaultHandle(pub Signal<Option<FileSystemDirectoryHandle>>);

/// App-scope context: the (Store, Persistence) pair returned by
/// `init_wasm_local_mode` once the vault is picked.
#[derive(Clone)]
pub struct WasmLocalContext(pub Signal<Option<WasmLocalReady>>);

/// Resolved Local Mode context. Cloning the Arc is cheap.
#[derive(Clone)]
pub struct WasmLocalReady {
    pub store: Store,
    pub persistence: Arc<dyn Persistence>,
    pub project_repo: Arc<dyn LocalProjectRepository>,
    pub note_repo: Arc<dyn LocalNoteRepository>,
    pub settings_repo: Arc<dyn LocalSettingsRepository>,
}

/// Stub of the desktop-only LocalSaveAction so cross-target lookups still
/// type-check. The wasm editor calls Persistence::save through the
/// resolved WasmLocalReady.
#[derive(Clone, PartialEq)]
pub struct LocalSaveAction {
    pub callback: Callback<()>,
}

/// Stub: settings panel toggle (parity with desktop).
#[derive(Clone, Copy)]
pub struct SettingsOpen(pub Signal<bool>);

/// Stub: latest Local username (currently unused in the wasm v1 shell).
#[derive(Clone, Copy)]
pub struct LocalUsername(pub Signal<String>);

/// Provider component: empty on wasm — context installation happens in
/// `provide_local_state` below so the same boot flow as desktop applies.
#[component]
pub fn LocalStateProvider(children: Element) -> Element {
    rsx! { {children} }
}

/// Install wasm Local Mode contexts. Called from `app.rs`.
pub fn provide_local_state() {
    let vault: Signal<Option<FileSystemDirectoryHandle>> = use_signal(|| None);
    use_context_provider(|| WasmVaultHandle(vault));
    let ctx: Signal<Option<WasmLocalReady>> = use_signal(|| None);
    use_context_provider(|| WasmLocalContext(ctx));
    let opened: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| OpenedNote(opened));
}

pub fn provide_local_app_signals() {
    let username: Signal<String> = use_signal(|| "Local user".to_string());
    use_context_provider(|| LocalUsername(username));
    let settings_open: Signal<bool> = use_signal(|| false);
    use_context_provider(|| SettingsOpen(settings_open));
    // Save callback: forwards to whatever the active editor wants to save.
    let cb: Callback<()> = Callback::new(move |_| {
        // No-op at app scope; the per-tab editor wires its own save.
    });
    use_context_provider(|| LocalSaveAction { callback: cb });
}

pub fn read_remembered_mode_web() -> Option<Mode> {
    Some(Mode::Local)
}

#[component]
pub fn LocalShellOverlay(children: Element) -> Element {
    let WasmVaultHandle(vault) = use_context();
    let WasmLocalContext(mut ctx) = use_context();

    // First-run vault picker. When no vault is set, render a button
    // that triggers `Window::show_directory_picker()`. On success, run
    // `init_wasm_local_mode` and stash the result in WasmLocalContext.
    if vault.read().is_none() {
        return rsx! {
            VaultPickerWasm { vault, ctx }
        };
    }

    // Init not done yet (vault picked but async chain hasn't completed) →
    // show a transient "loading" panel.
    if ctx.read().is_none() {
        let vault_handle = vault.read().clone();
        spawn(async move {
            let Some(handle) = vault_handle else { return };
            match wasm_init::init_wasm_local_mode(&handle).await {
                Ok(init) => {
                    let store = init.store;
                    let project_repo: Arc<dyn LocalProjectRepository> =
                        Arc::new(SqliteLocalProjectRepository::new(store.clone()));
                    let note_repo: Arc<dyn LocalNoteRepository> =
                        Arc::new(SqliteLocalNoteRepository::new(store.clone()));
                    let settings_repo: Arc<dyn LocalSettingsRepository> =
                        Arc::new(SqliteLocalSettingsRepository::new(store.clone()));
                    ctx.set(Some(WasmLocalReady {
                        store,
                        persistence: init.persistence,
                        project_repo,
                        note_repo,
                        settings_repo,
                    }));
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("operon: local-mode init: {e}").into());
                }
            }
        });
        return rsx! {
            div {
                class: "operon-modal-scrim",
                "data-testid": "local-shell-loading",
                div {
                    class: "operon-modal-card",
                    p { "Opening vault…" }
                }
            }
        };
    }

    rsx! {
        div {
            class: "operon-local-shell",
            "data-testid": "local-shell",
            style: "display: flex; flex-direction: row; height: 100vh; width: 100vw;",
            ExplorerPanel {}
            LocalNoteEditor { tab_id: TabId(0), action: use_context::<LocalSaveAction>() }
            {children}
        }
    }
}

/// Vault picker for wasm. Single button that triggers
/// `Window::show_directory_picker()` and stashes the resolved handle.
#[component]
fn VaultPickerWasm(
    vault: Signal<Option<FileSystemDirectoryHandle>>,
    ctx: Signal<Option<WasmLocalReady>>,
) -> Element {
    let _ = ctx;
    let mut vault = vault;
    let error: Signal<Option<String>> = use_signal(|| None);
    let pick = move |_| {
        let mut error_setter = error;
        spawn(async move {
            let Some(window) = web_sys::window() else {
                error_setter.set(Some("no window".into()));
                return;
            };
            // Try previously stored OPFS handle first.
            if let Ok(Some(stored)) = super::web_vault_handle::load_handle().await {
                vault.set(Some(stored));
                return;
            }
            // Otherwise call showDirectoryPicker on the Window object.
            let picker_method = match js_sys::Reflect::get(
                &window,
                &wasm_bindgen::JsValue::from_str("showDirectoryPicker"),
            ) {
                Ok(v) if v.is_function() => v,
                _ => {
                    error_setter.set(Some(
                        "showDirectoryPicker unavailable (use a Chromium-based browser)".into(),
                    ));
                    return;
                }
            };
            let func: js_sys::Function = picker_method.unchecked_into();
            let promise = match func.call0(&window) {
                Ok(p) => js_sys::Promise::from(p),
                Err(e) => {
                    error_setter.set(Some(format!("showDirectoryPicker call: {e:?}")));
                    return;
                }
            };
            let value = match JsFuture::from(promise).await {
                Ok(v) => v,
                Err(e) => {
                    error_setter.set(Some(format!("picker rejected: {e:?}")));
                    return;
                }
            };
            let handle: FileSystemDirectoryHandle = match value.dyn_into() {
                Ok(h) => h,
                Err(v) => {
                    error_setter.set(Some(format!("not a directory handle: {v:?}")));
                    return;
                }
            };
            // Persist the handle so the next reload skips this picker.
            let _ = super::web_vault_handle::store_handle(&handle).await;
            vault.set(Some(handle));
        });
    };

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "wasm-vault-picker",
            div {
                class: "operon-modal-card",
                h2 { class: "operon-modal-title", "Choose your notes vault" }
                p { class: "operon-modal-help",
                    "Operon will store SQLite metadata + note bodies under the chosen folder via OPFS."
                }
                if let Some(msg) = error.read().clone() {
                    p { role: "alert", class: "operon-modal-error", "{msg}" }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "wasm-vault-pick",
                        onclick: pick,
                        "Choose folder…"
                    }
                }
            }
        }
    }
}

/// V1 wasm explorer: lists projects + their notes; clicking a note opens
/// it in the editor pane via the `OPENED_NOTE` signal below.
#[component]
pub fn ExplorerPanel() -> Element {
    let WasmLocalContext(ctx) = use_context();
    let opened: Signal<Option<Uuid>> = use_context::<OpenedNote>().0;
    let mut opened_setter = opened;

    let Some(ready) = ctx.read().clone() else {
        return rsx! { aside { "Loading…" } };
    };

    let projects = ready.project_repo.list().unwrap_or_default();
    let project_repo = ready.project_repo.clone();
    let mut new_project_name: Signal<String> = use_signal(String::new);

    let note_repo_for_render = ready.note_repo.clone();

    rsx! {
        aside {
            class: "operon-local-explorer",
            "data-testid": "explorer-panel",
            style: "width: 280px; padding: 0.5rem; border-right: 1px solid var(--operon-border, #ccc); overflow-y: auto;",
            h2 { style: "font-size: 1rem;", "Projects" }
            div {
                style: "display: flex; gap: 0.25rem; margin-bottom: 0.5rem;",
                input {
                    r#type: "text",
                    placeholder: "New project name",
                    value: "{new_project_name.read()}",
                    style: "flex: 1;",
                    oninput: move |evt| new_project_name.set(evt.value()),
                }
                button {
                    r#type: "button",
                    onclick: move |_| {
                        let name = new_project_name.read().trim().to_string();
                        if name.is_empty() { return; }
                        let _ = project_repo.create(&name);
                        new_project_name.set(String::new());
                    },
                    "Add"
                }
            }
            ul {
                style: "list-style: none; padding: 0; margin: 0;",
                for p in projects.iter().cloned() {
                    li {
                        key: "p{p.id}",
                        "data-testid": "project-row",
                        "data-project-id": "{p.id}",
                        details {
                            open: true,
                            summary { style: "font-weight: 600;", "{p.name}" }
                            {
                                let note_repo = note_repo_for_render.clone();
                                let notes = note_repo.list_for_project(p.id).unwrap_or_default();
                                let pid = p.id;
                                let note_repo_for_add = note_repo.clone();
                                rsx! {
                                    button {
                                        r#type: "button",
                                        style: "font-size: 0.85em; margin: 0.25rem 0;",
                                        onclick: move |_| {
                                            let _ = note_repo_for_add.create(pid, None, "");
                                        },
                                        "+ note"
                                    }
                                    ul {
                                        style: "list-style: none; padding-left: 0.5rem;",
                                        for n in notes.into_iter() {
                                            li {
                                                key: "n{n.id}",
                                                "data-testid": "note-row",
                                                "data-note-id": "{n.id}",
                                                style: "cursor: pointer; padding: 0.15rem 0.25rem;",
                                                onclick: move |_| {
                                                    opened_setter.set(Some(n.id));
                                                },
                                                "{n.title}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// App-scope: which note is currently open in the editor pane.
#[derive(Clone, Copy)]
pub struct OpenedNote(pub Signal<Option<Uuid>>);

#[component]
pub fn LocalNoteEditor(tab_id: TabId, action: LocalSaveAction) -> Element {
    let _ = (tab_id, action);
    let WasmLocalContext(ctx) = use_context();
    let OpenedNote(opened) = use_context();
    let mut content: Signal<String> = use_signal(String::new);
    let current_id: Signal<Option<Uuid>> = use_signal(|| None);
    let opened_now = *opened.read();

    // Load body when the active note changes.
    {
        let ctx_snap = ctx.read().clone();
        let mut content = content;
        let mut current_id = current_id;
        use_effect(move || {
            let id = *opened.read();
            if id == *current_id.read() {
                return;
            }
            current_id.set(id);
            let Some(id) = id else { return; };
            let Some(ready) = ctx_snap.clone() else { return };
            let pers = ready.persistence.clone();
            spawn(async move {
                match pers.load(&id.to_string()).await {
                    Ok(bytes) => {
                        let s = String::from_utf8_lossy(&bytes).to_string();
                        content.set(s);
                    }
                    Err(_) => content.set(String::new()),
                }
            });
        });
    }

    let Some(id) = opened_now else {
        return rsx! {
            section {
                class: "operon-local-editor-empty",
                "data-testid": "editor-empty",
                style: "flex: 1; display: flex; align-items: center; justify-content: center; opacity: 0.5;",
                "Pick a note from the left."
            }
        };
    };

    let save = {
        let ctx_snap = ctx.read().clone();
        let content = content;
        move |_| {
            let Some(ready) = ctx_snap.clone() else { return };
            let body = content.read().clone();
            let pers = ready.persistence.clone();
            let note_repo = ready.note_repo.clone();
            spawn(async move {
                let _ = pers.save(&id.to_string(), body.as_bytes()).await;
                let _ = note_repo.touch_updated(id);
            });
        }
    };

    rsx! {
        section {
            class: "operon-local-editor",
            "data-testid": "local-editor",
            style: "flex: 1; display: flex; flex-direction: column; padding: 0.5rem;",
            div {
                style: "display: flex; gap: 0.5rem; margin-bottom: 0.25rem;",
                button {
                    r#type: "button",
                    "data-testid": "wasm-save",
                    onclick: save,
                    "Save"
                }
            }
            textarea {
                "data-testid": "wasm-textarea",
                style: "flex: 1; font-family: monospace; font-size: 0.9em; padding: 0.5rem;",
                value: "{content.read()}",
                oninput: move |evt| content.set(evt.value()),
            }
        }
    }
}

#[component]
pub fn StartupChooser() -> Element {
    // On wasm we go straight into Local Mode — there's no Cloud RBAG path on web.
    rsx! {
        div {
            class: "flex items-center justify-center h-screen w-screen text-sm",
            "data-testid": "mode-chooser",
            "Loading Local Mode…"
        }
    }
}
