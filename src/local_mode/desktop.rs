//! Desktop (non-wasm) implementation of Local Mode UI + repo wiring.

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::{
    LocalNoteRepository, LocalProjectRepository, LocalSearchRepository, LocalSettingsRepository,
    LocalTreeStateRepository, LocalUserRepository, SqliteLocalNoteRepository,
    SqliteLocalProjectRepository, SqliteLocalSearchRepository, SqliteLocalSettingsRepository,
    SqliteLocalTreeStateRepository, SqliteLocalUserRepository,
};
use operon_store::{Store, StoreConfig};
use uuid::Uuid;

use super::editor::{install_save_action, LocalNoteEditor, LocalSaveAction, LocalSaveButton};
use super::explorer::{
    ExplorerPanel, ExplorerSearchFocus, ExplorerSearchRepo, LocalNoteVersion, LocalProjectVersion,
    SelectedNote, SelectedProject,
};
use super::ui::{ClipKind, ClipPayload, Clipboard, DragKind, DragSession, LocalClipboard};
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
        Arc::new(SqliteLocalTreeStateRepository::new(store.clone()));
    let search_repo: Arc<dyn LocalSearchRepository> =
        Arc::new(SqliteLocalSearchRepository::new(store));
    use_context_provider(|| LocalUserRepo(user_repo));
    use_context_provider(|| LocalSettingsRepo(settings_repo));
    use_context_provider(|| LocalProjectRepo(project_repo));
    use_context_provider(|| LocalNoteRepo(note_repo));
    use_context_provider(|| LocalTreeStateRepo(tree_repo));
    use_context_provider(|| ExplorerSearchRepo(search_repo));
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

/// App-scope signal: gear → settings panel toggle. Lives at App scope so the
/// ActivityBar gear (rendered inside Cloud `Shell`) and the overlay can share it.
#[derive(Clone, Copy)]
pub struct SettingsOpen(pub Signal<bool>);

/// App-scope signal: latest Local username. StatusBar reads it; SettingsPanel
/// updates it on save. Seeded from `LocalUserRepo::get()`; falls back to
/// "Local user" with an upsert when the row is empty.
#[derive(Clone, Copy)]
pub struct LocalUsername(pub Signal<String>);

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
        Arc::new(SqliteLocalTreeStateRepository::new(store.clone()));
    let search_repo: Arc<dyn LocalSearchRepository> =
        Arc::new(SqliteLocalSearchRepository::new(store));
    use_context_provider(|| LocalUserRepo(user_repo));
    use_context_provider(|| LocalSettingsRepo(settings_repo));
    use_context_provider(|| LocalProjectRepo(project_repo));
    use_context_provider(|| LocalNoteRepo(note_repo));
    use_context_provider(|| LocalTreeStateRepo(tree_repo));
    use_context_provider(|| ExplorerSearchRepo(search_repo));
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
    // Phase-4: drag session + clipboard live at app scope so explorer rows can
    // co-ordinate without prop-drilling.
    let drag_session: Signal<Option<DragKind>> = use_signal(|| None);
    use_context_provider(|| DragSession(drag_session));
    let clipboard: Signal<Option<Clipboard>> = use_signal(|| None);
    use_context_provider(|| LocalClipboard(clipboard));
    // Phase-5: bumped by Ctrl+Shift+F so the explorer search input refocuses.
    let search_focus_tick: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| ExplorerSearchFocus(search_focus_tick));

    // Explicit-save action — the Save button + Ctrl+S call this.
    let save_callback =
        use_hook(|| install_save_action(tabs, persistence.clone(), note_repo.clone()));
    let save_action = LocalSaveAction {
        callback: save_callback,
    };
    use_context_provider(|| save_action.clone());

    let active_tab_id = tabs.read().active_id();

    // Capture the repos used by the paste keyboard handler.
    let note_repo_for_keys = note_repo.clone();
    let project_repo_for_keys: Arc<dyn LocalProjectRepository> = {
        let LocalProjectRepo(r) = use_context();
        r
    };
    let mut clipboard_setter = clipboard;
    let mut selected_project_setter = selected_project;
    let mut note_version_setter = note_version;
    let project_repo_keys = project_repo_for_keys.clone();
    let mut search_focus_tick_setter = search_focus_tick;

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
                    return;
                }
                // Phase-5: Ctrl+Shift+F focuses the explorer search input. The
                // existing Ctrl+P binding (cloud command palette) lives in the
                // shell module and is left untouched.
                if with_meta
                    && mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && evt.key().to_string().eq_ignore_ascii_case("f")
                {
                    evt.prevent_default();
                    search_focus_tick_setter.with_mut(|t| *t += 1);
                    return;
                }
                if with_meta && !mods.contains(Modifiers::ALT) {
                    let key = evt.key().to_string();
                    if key.eq_ignore_ascii_case("x") || key.eq_ignore_ascii_case("c") {
                        // Cut/Copy require a selected note (or project as fallback).
                        let payload = if let Some(nid) = *selected_note.read() {
                            Some(ClipPayload::Note(nid))
                        } else {
                            (*selected_project.read()).map(ClipPayload::Project)
                        };
                        if let Some(payload) = payload {
                            let kind = if key.eq_ignore_ascii_case("x") {
                                ClipKind::Cut
                            } else {
                                ClipKind::Copy
                            };
                            clipboard_setter.set(Some(Clipboard { kind, payload }));
                            evt.prevent_default();
                            return;
                        }
                    }
                    if key.eq_ignore_ascii_case("v") {
                        let clip = *clipboard.read();
                        if let Some(clip) = clip {
                            paste_clipboard(
                                clip,
                                *selected_note.read(),
                                *selected_project.read(),
                                &note_repo_for_keys,
                                &project_repo_keys,
                            );
                            note_version_setter.with_mut(|v| *v += 1);
                            if matches!(clip.kind, ClipKind::Cut) {
                                clipboard_setter.set(None);
                            }
                            evt.prevent_default();
                            return;
                        }
                    }
                }
                if evt.key().to_string() == "Escape" && clipboard.read().is_some() {
                    clipboard_setter.set(None);
                    evt.prevent_default();
                    return;
                }
                // Make warnings about unused mut go away when neither cut nor paste fires.
                let _ = &mut selected_project_setter;
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

/// Resolve a Paste action: cut moves a note (or project) into the selected
/// target; copy duplicates the subtree. Targets follow the Phase-4 rules:
/// selected note → child of that note; selected project (no note) → last
/// root-level note in that project; nothing selected → last root-level note
/// in the first project.
fn paste_clipboard(
    clip: Clipboard,
    selected_note: Option<Uuid>,
    selected_project: Option<Uuid>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
) {
    // Resolve the destination project + parent.
    let (dest_project, dest_parent) = if let Some(note_id) = selected_note {
        // Need to find the project that owns this note. Easiest: probe each project.
        let projects = project_repo.list().unwrap_or_default();
        let owning = projects
            .iter()
            .find(|p| {
                note_repo
                    .list_for_project(p.id)
                    .ok()
                    .map(|rows| rows.iter().any(|r| r.id == note_id))
                    .unwrap_or(false)
            })
            .map(|p| p.id);
        match owning {
            Some(pid) => (pid, Some(note_id)),
            None => return,
        }
    } else if let Some(pid) = selected_project {
        (pid, None)
    } else {
        let projects = project_repo.list().unwrap_or_default();
        match projects.first() {
            Some(p) => (p.id, None),
            None => return,
        }
    };

    let last_index = match dest_parent {
        Some(pid) => note_repo
            .list_for_project(dest_project)
            .map(|rows| rows.iter().filter(|r| r.parent_id == Some(pid)).count() as i64)
            .unwrap_or(0),
        None => note_repo
            .list_for_project(dest_project)
            .map(|rows| rows.iter().filter(|r| r.parent_id.is_none()).count() as i64)
            .unwrap_or(0),
    };

    match (clip.kind, clip.payload) {
        (ClipKind::Cut, ClipPayload::Note(nid)) => {
            if let Err(e) = note_repo.move_to(nid, dest_project, dest_parent, last_index) {
                eprintln!("operon: paste cut note failed: {e}");
            }
        }
        (ClipKind::Copy, ClipPayload::Note(nid)) => {
            if let Err(e) = note_repo.duplicate_subtree(nid, dest_project, dest_parent, last_index)
            {
                eprintln!("operon: paste copy note failed: {e}");
            }
        }
        (_, ClipPayload::Project(_)) => {
            // Project cut/copy via keyboard is reserved for future phases — the
            // explorer ignores it for now (clipboard will be cleared by the
            // caller when needed).
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

/// Lift every Local-Mode app-scope signal to App scope so the Cloud `Shell`
/// chrome (mode-aware StatusBar / ActivityBar / SideBar plugin contributions)
/// can read them without prop-drilling. Call from `app.rs` only when
/// `Mode::Local`, after `provide_local_state()`.
pub fn provide_local_app_signals() {
    let LocalUserRepo(user_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let tabs: Signal<TabManager> = use_context();

    // Seed username from the DB; upsert a default row when missing so the
    // status bar always renders a value.
    let mut username: Signal<String> = use_signal(|| {
        user_repo
            .get()
            .ok()
            .flatten()
            .map(|u| u.username)
            .unwrap_or_else(|| "Local user".to_string())
    });
    let user_repo_for_seed = user_repo.clone();
    use_hook(move || {
        if let Ok(None) = user_repo_for_seed.get() {
            if let Ok(seeded) = user_repo_for_seed.upsert("Local user") {
                username.set(seeded.username);
            }
        }
    });
    use_context_provider(|| LocalUsername(username));

    let settings_open: Signal<bool> = use_signal(|| false);
    use_context_provider(|| SettingsOpen(settings_open));

    let project_version: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| LocalProjectVersion(project_version));
    let note_version: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| LocalNoteVersion(note_version));
    let selected_project: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| SelectedProject(selected_project));
    let selected_note: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| SelectedNote(selected_note));
    let drag_session: Signal<Option<DragKind>> = use_signal(|| None);
    use_context_provider(|| DragSession(drag_session));
    let clipboard: Signal<Option<Clipboard>> = use_signal(|| None);
    use_context_provider(|| LocalClipboard(clipboard));
    let search_focus_tick: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| ExplorerSearchFocus(search_focus_tick));

    let save_callback =
        use_hook(|| install_save_action(tabs, persistence.clone(), note_repo.clone()));
    use_context_provider(|| LocalSaveAction {
        callback: save_callback,
    });
}

/// Wraps the Cloud `Shell` for Local Mode. Owns the Local-only keyboard
/// bindings (Ctrl+X / Ctrl+C / Ctrl+V / Esc-clear-clip / Ctrl+Shift+F),
/// renders the `SettingsPanel` overlay when `SettingsOpen` flips on, and
/// surfaces a small floating Save button for `manual_save` tabs.
#[component]
pub fn LocalShellOverlay(children: Element) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    let LocalClipboard(clipboard) = use_context();
    let SelectedProject(selected_project) = use_context();
    let SelectedNote(selected_note) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let ExplorerSearchFocus(search_focus_tick) = use_context();
    let SettingsOpen(settings_open) = use_context();
    let LocalUsername(username) = use_context();
    let tabs: Signal<TabManager> = use_context();
    let save_action: LocalSaveAction = use_context();

    let mut clipboard_setter = clipboard;
    let mut selected_project_setter = selected_project;
    let mut note_version_setter = note_version;
    let mut search_focus_tick_setter = search_focus_tick;
    let _ = username;
    let _ = settings_open;
    let note_repo_for_keys = note_repo.clone();
    let project_repo_for_keys = project_repo.clone();

    let active_tab_dirty_and_manual = {
        let tm = tabs.read();
        tm.active().map(|t| (t.id, t.dirty, t.manual_save))
    };

    rsx! {
        div {
            tabindex: "-1",
            "data-testid": "local-shell",
            style: "display: contents;",
            onkeydown: move |evt| {
                let mods = evt.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                let key = evt.key().to_string();
                if with_meta
                    && mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && key.eq_ignore_ascii_case("f")
                {
                    evt.prevent_default();
                    search_focus_tick_setter.with_mut(|t| *t += 1);
                    return;
                }
                if with_meta && !mods.contains(Modifiers::ALT) && !mods.contains(Modifiers::SHIFT) {
                    if key.eq_ignore_ascii_case("x") || key.eq_ignore_ascii_case("c") {
                        let payload = if let Some(nid) = *selected_note.read() {
                            Some(ClipPayload::Note(nid))
                        } else {
                            (*selected_project.read()).map(ClipPayload::Project)
                        };
                        if let Some(payload) = payload {
                            let kind = if key.eq_ignore_ascii_case("x") {
                                ClipKind::Cut
                            } else {
                                ClipKind::Copy
                            };
                            clipboard_setter.set(Some(Clipboard { kind, payload }));
                            evt.prevent_default();
                            return;
                        }
                    }
                    if key.eq_ignore_ascii_case("v") {
                        let clip = *clipboard.read();
                        if let Some(clip) = clip {
                            paste_clipboard(
                                clip,
                                *selected_note.read(),
                                *selected_project.read(),
                                &note_repo_for_keys,
                                &project_repo_for_keys,
                            );
                            note_version_setter.with_mut(|v| *v += 1);
                            if matches!(clip.kind, ClipKind::Cut) {
                                clipboard_setter.set(None);
                            }
                            evt.prevent_default();
                            return;
                        }
                    }
                }
                if key == "Escape" && clipboard.read().is_some() {
                    clipboard_setter.set(None);
                    evt.prevent_default();
                    return;
                }
                let _ = &mut selected_project_setter;
            },
            {children}
            // Floating Save button: only for Local-Mode tabs (manual_save = true).
            if let Some((_, dirty, true)) = active_tab_dirty_and_manual {
                div {
                    style: "position: fixed; top: 36px; right: 12px; z-index: 40;",
                    LocalSaveButton { action: save_action.clone(), dirty }
                }
            }
            if *settings_open.read() {
                SettingsPanel {
                    open: settings_open,
                    username,
                }
            }
        }
    }
}
