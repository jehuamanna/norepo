//! Desktop (non-wasm) implementation of Local Mode UI + repo wiring.

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::{
    LocalNoteRepository, LocalProjectRepository, LocalSettingsRepository, LocalTreeStateRepository,
    LocalUserRepository, SqliteLocalNoteRepository, SqliteLocalProjectRepository,
    SqliteLocalSettingsRepository, SqliteLocalTreeStateRepository, SqliteLocalUserRepository,
};
use operon_store::{Store, StoreConfig};
use uuid::Uuid;

use super::editor::{install_save_action, LocalNoteEditor, LocalSaveAction};
use super::explorer::{
    ExplorerPanel, LocalNoteVersion, LocalProjectVersion, SelectedNote, SelectedProject,
};
use super::{MODE_VALUE_CLOUD, MODE_VALUE_LOCAL, SETTINGS_KEY_MODE_REMEMBERED};
use crate::persistence::Persistence;
use crate::rbag::state::{AppState, Mode};
use crate::tabs::{SaveScheduler, TabManager};

/// Provider component: mounts a [`Store`] for Local Mode and exposes the
/// repository trait objects via context. Mount near the app root.
#[component]
pub fn LocalStateProvider(children: Element) -> Element {
    let store = use_hook(open_local_store);
    let user_repo: Arc<dyn LocalUserRepository> =
        Arc::new(SqliteLocalUserRepository::new(store.clone()));
    let settings_repo: Arc<dyn LocalSettingsRepository> =
        Arc::new(SqliteLocalSettingsRepository::new(store.clone()));
    let project_repo: Arc<dyn LocalProjectRepository> =
        Arc::new(SqliteLocalProjectRepository::new(store.clone()));
    let note_repo: Arc<dyn LocalNoteRepository> =
        Arc::new(SqliteLocalNoteRepository::new(store.clone()));
    let tree_repo: Arc<dyn LocalTreeStateRepository> =
        Arc::new(SqliteLocalTreeStateRepository::new(store));
    use_context_provider(|| LocalUserRepo(user_repo));
    use_context_provider(|| LocalSettingsRepo(settings_repo));
    use_context_provider(|| LocalProjectRepo(project_repo));
    use_context_provider(|| LocalNoteRepo(note_repo));
    use_context_provider(|| LocalTreeStateRepo(tree_repo));
    rsx! { {children} }
}

/// Newtype wrappers for context lookup. Dioxus's context system keys by type;
/// wrapping the trait objects keeps the lookup unambiguous.
#[derive(Clone)]
pub struct LocalUserRepo(pub Arc<dyn LocalUserRepository>);

#[derive(Clone)]
pub struct LocalSettingsRepo(pub Arc<dyn LocalSettingsRepository>);

#[derive(Clone)]
pub struct LocalProjectRepo(pub Arc<dyn LocalProjectRepository>);

#[derive(Clone)]
pub struct LocalNoteRepo(pub Arc<dyn LocalNoteRepository>);

#[derive(Clone)]
pub struct LocalTreeStateRepo(pub Arc<dyn LocalTreeStateRepository>);

/// Convenience used by `app.rs` and tests to install the repos. Equivalent to
/// rendering [`LocalStateProvider`] but callable from a hook position.
pub fn provide_local_state() {
    let store = open_local_store();
    let user_repo: Arc<dyn LocalUserRepository> =
        Arc::new(SqliteLocalUserRepository::new(store.clone()));
    let settings_repo: Arc<dyn LocalSettingsRepository> =
        Arc::new(SqliteLocalSettingsRepository::new(store.clone()));
    let project_repo: Arc<dyn LocalProjectRepository> =
        Arc::new(SqliteLocalProjectRepository::new(store.clone()));
    let note_repo: Arc<dyn LocalNoteRepository> =
        Arc::new(SqliteLocalNoteRepository::new(store.clone()));
    let tree_repo: Arc<dyn LocalTreeStateRepository> =
        Arc::new(SqliteLocalTreeStateRepository::new(store));
    use_context_provider(|| LocalUserRepo(user_repo));
    use_context_provider(|| LocalSettingsRepo(settings_repo));
    use_context_provider(|| LocalProjectRepo(project_repo));
    use_context_provider(|| LocalNoteRepo(note_repo));
    use_context_provider(|| LocalTreeStateRepo(tree_repo));
}

fn open_local_store() -> Store {
    let path = default_store_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    Store::open(StoreConfig::local(&path))
        .or_else(|e| {
            eprintln!("operon: failed to open local store at {path:?} ({e}); using :memory:");
            Store::open_in_memory()
        })
        .expect("local store: in-memory fallback must succeed")
}

fn default_store_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".local/share/operon/local.sqlite");
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return std::path::PathBuf::from(home).join("AppData/Local/operon/local.sqlite");
    }
    std::env::temp_dir().join("operon/local.sqlite")
}

/// Top-level Local-Mode shell. Phase-1 renders the badge + settings gear and a
/// placeholder workspace; later phases fill the centre. Phase-3 mounts the
/// explicit-save action and an editor surface for the active Local-Mode tab.
#[component]
pub fn LocalShell() -> Element {
    let LocalUserRepo(user_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let tabs: Signal<TabManager> = use_context();
    let _scheduler: SaveScheduler = use_context();

    let mut username: Signal<String> = use_signal(|| {
        user_repo
            .get()
            .ok()
            .flatten()
            .map(|u| u.username)
            .unwrap_or_else(|| "Local user".to_string())
    });
    // Seed a default row so the badge always reflects DB state on subsequent reads.
    let user_repo_for_seed = user_repo.clone();
    use_hook(move || {
        if let Ok(None) = user_repo_for_seed.get() {
            if let Ok(seeded) = user_repo_for_seed.upsert("Local user") {
                username.set(seeded.username);
            }
        }
    });

    let mut settings_open: Signal<bool> = use_signal(|| false);

    // App-scope explorer state. Provided here (the LocalShell root) so the
    // explorer panel and any future toolbar / status surface share it.
    let project_version: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| LocalProjectVersion(project_version));
    let note_version: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| LocalNoteVersion(note_version));
    let selected_project: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| SelectedProject(selected_project));
    let selected_note: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| SelectedNote(selected_note));

    // Explicit-save action — the Save button + Ctrl+S call this.
    let save_callback =
        use_hook(|| install_save_action(tabs, persistence.clone(), note_repo.clone()));
    let save_action = LocalSaveAction {
        callback: save_callback,
    };
    use_context_provider(|| save_action.clone());

    let active_tab_id = tabs.read().active_id();

    rsx! {
        div {
            class: "flex flex-col h-screen w-screen bg-[var(--operon-bg)] text-[var(--operon-fg)]",
            "data-testid": "local-shell",
            tabindex: "-1",
            onkeydown: move |evt| {
                let mods = evt.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                if with_meta
                    && !mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && evt.key().to_string().eq_ignore_ascii_case("s")
                {
                    evt.prevent_default();
                    save_action.callback.call(());
                }
            },
            // Top bar
            div {
                class: "flex items-center justify-end px-4 py-2 border-b border-[var(--operon-border)]",
                TopRightBadge {
                    org_label: "Local".to_string(),
                    username: username.read().clone(),
                }
            }
            // Sidebar + workspace.
            div {
                class: "flex-1 flex min-h-0",
                aside {
                    class: "w-64 shrink-0 flex flex-col min-h-0",
                    ExplorerPanel {}
                }
                main {
                    class: "flex-1 flex flex-col min-h-0",
                    if let Some(tab_id) = active_tab_id {
                        LocalNoteEditor { tab_id, action: save_action.clone() }
                    } else {
                        div {
                            class: "flex-1 flex items-center justify-center text-sm opacity-70",
                            "data-testid": "local-empty-workspace",
                            "Pick a note to start writing."
                        }
                    }
                }
            }
            // Bottom bar
            div {
                class: "flex items-center justify-start px-4 py-2 border-t border-[var(--operon-border)]",
                SettingsGear {
                    on_click: move |_| settings_open.set(true),
                }
            }
            if *settings_open.read() {
                SettingsPanel { open: settings_open, username: username }
            }
        }
    }
}

#[component]
pub fn TopRightBadge(org_label: String, username: String) -> Element {
    rsx! {
        div {
            class: "flex items-center gap-2 px-3 py-1 rounded-full border border-[var(--operon-border)] text-xs",
            "data-testid": "top-right-badge",
            span { class: "font-semibold", "{org_label}" }
            span { class: "opacity-60", "/" }
            span { "{username}" }
        }
    }
}

#[component]
pub fn SettingsGear(on_click: EventHandler<MouseEvent>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "w-8 h-8 inline-flex items-center justify-center rounded hover:bg-[var(--operon-hover)] text-lg",
            "data-testid": "settings-gear",
            "aria-label": "Open settings",
            onclick: move |evt| on_click.call(evt),
            "\u{2699}"
        }
    }
}

#[component]
pub fn SettingsPanel(open: Signal<bool>, username: Signal<String>) -> Element {
    let LocalUserRepo(user_repo) = use_context();
    let mut draft: Signal<String> = use_signal(|| username.read().clone());
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let mut close = move || {
        open.set(false);
    };

    let user_repo_for_save = user_repo.clone();
    let mut save = move || {
        let value = draft.read().clone();
        match user_repo_for_save.upsert(&value) {
            Ok(saved) => {
                username.set(saved.username);
                error.set(None);
                open.set(false);
            }
            Err(e) => error.set(Some(e.to_string())),
        }
    };

    rsx! {
        div {
            class: "fixed inset-0 bg-black/40 flex items-center justify-center z-50",
            "data-testid": "settings-panel",
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    close();
                    evt.prevent_default();
                }
            },
            div {
                class: "bg-[var(--operon-bg)] text-[var(--operon-fg)] border border-[var(--operon-border)] rounded-md p-4 w-80 shadow-lg",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "text-sm font-semibold mb-3", "Local user" }
                label {
                    class: "block text-xs mb-1 opacity-70",
                    "Username"
                }
                input {
                    r#type: "text",
                    class: "w-full px-2 py-1 mb-2 bg-[var(--operon-input-bg)] border border-[var(--operon-border)] rounded text-sm",
                    "data-testid": "username-input",
                    value: "{draft.read()}",
                    autofocus: true,
                    oninput: move |evt| draft.set(evt.value()),
                }
                if let Some(msg) = error.read().clone() {
                    p { class: "text-xs text-red-500 mb-2", "{msg}" }
                }
                div {
                    class: "flex justify-end gap-2 mt-2",
                    button {
                        r#type: "button",
                        class: "px-3 py-1 text-xs rounded border border-[var(--operon-border)]",
                        onclick: move |_| close(),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "px-3 py-1 text-xs rounded bg-[var(--operon-accent)] text-white",
                        onclick: move |_| save(),
                        "Save"
                    }
                }
            }
        }
    }
}

/// Two-button chooser shown on first launch (or whenever
/// `local_app_settings.mode_remembered` is empty).
#[component]
pub fn StartupChooser() -> Element {
    let LocalSettingsRepo(settings_repo) = use_context();
    let mut state = use_context::<Signal<AppState>>();

    let pick_local = {
        let settings = settings_repo.clone();
        move |_| {
            let _ = settings.set(SETTINGS_KEY_MODE_REMEMBERED, MODE_VALUE_LOCAL);
            state.with_mut(|s| s.mode = Mode::Local);
        }
    };
    let pick_cloud = {
        let settings = settings_repo.clone();
        move |_| {
            let _ = settings.set(SETTINGS_KEY_MODE_REMEMBERED, MODE_VALUE_CLOUD);
            state.with_mut(|s| s.mode = Mode::NonLocal);
        }
    };

    rsx! {
        div {
            class: "flex flex-col items-center justify-center h-screen w-screen gap-6 bg-[var(--operon-bg)] text-[var(--operon-fg)]",
            "data-testid": "mode-chooser",
            h1 { class: "text-lg font-semibold", "Choose how to run Operon" }
            div {
                class: "flex gap-4",
                button {
                    r#type: "button",
                    class: "px-8 py-6 rounded-md border border-[var(--operon-border)] hover:bg-[var(--operon-hover)] text-base font-medium",
                    "data-testid": "chooser-local",
                    onclick: pick_local,
                    "Local"
                }
                button {
                    r#type: "button",
                    class: "px-8 py-6 rounded-md border border-[var(--operon-border)] hover:bg-[var(--operon-hover)] text-base font-medium",
                    "data-testid": "chooser-cloud",
                    onclick: pick_cloud,
                    "Cloud (RBAG)"
                }
            }
        }
    }
}

/// Read the persisted mode from `local_app_settings`. Used by `app.rs` to
/// decide whether to render the chooser or jump straight into a shell.
pub fn read_remembered_mode(settings: &Arc<dyn LocalSettingsRepository>) -> Option<Mode> {
    let raw = settings.get(SETTINGS_KEY_MODE_REMEMBERED).ok().flatten()?;
    match raw.as_str() {
        MODE_VALUE_LOCAL => Some(Mode::Local),
        MODE_VALUE_CLOUD => Some(Mode::NonLocal),
        _ => None,
    }
}
