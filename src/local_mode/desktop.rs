//! Desktop (non-wasm) implementation of Local Mode UI + repo wiring.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;

use dioxus::prelude::*;
use operon_store::repos::{
    LocalNoteLinkRepository, LocalNoteRepository, LocalProjectRepository, LocalSearchRepository,
    LocalSettingsRepository, LocalTreeStateRepository, LocalUserRepository, NoteKind,
    SqliteLocalNoteLinkRepository, SqliteLocalNoteRepository, SqliteLocalProjectRepository,
    SqliteLocalSearchRepository, SqliteLocalSettingsRepository, SqliteLocalTreeStateRepository,
    SqliteLocalUserRepository,
};
use operon_store::vfs;
use operon_store::{Store, StoreConfig};
use uuid::Uuid;

use super::editor::{install_save_action, LocalSaveAction};
use super::explorer::{
    ExplorerSearchRepo, LocalNoteVersion, LocalProjectVersion, SelectedNote, SelectedProject,
    TreeStateQueue, WorkspaceOpenMap, WorkspaceTreeQueueCtx,
};
use super::ui::{ClipKind, ClipPayload, Clipboard, DragKind, DragSession, LocalClipboard};
use super::{MODE_VALUE_CLOUD, MODE_VALUE_LOCAL, SETTINGS_KEY_MODE_REMEMBERED};
use crate::persistence::{
    fs::FilesystemWatcher, NoteWatcher, Persistence, WatchEvent, WatchHandle,
};
use crate::tabs::TabId;
use crate::plugins::artifact::revision_table;
use crate::rbag::state::{AppState, Mode};
use crate::tabs::TabManager;

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
    let raw_note_repo: Arc<dyn LocalNoteRepository> =
        Arc::new(SqliteLocalNoteRepository::new(store.clone()));
    // Migration 018: wrap the note repo so artifact renames / moves /
    // deletes also relocate the on-disk `<vault>/.operon/<project-id>/
    // artifacts/<slug>/.../` directory to match the UI tree. Vault is
    // snapshotted from settings here; first-run users who pick a vault
    // mid-session need a restart for relocation to start working (the
    // SQLite row is still source-of-truth in the meantime).
    let vault_snapshot = crate::local_mode::vault::load(&settings_repo)
        .ok()
        .flatten();
    let note_repo: Arc<dyn LocalNoteRepository> =
        Arc::new(crate::plugins::artifact::relocate::RelocatingNoteRepo::new(
            raw_note_repo,
            vault_snapshot,
        ));
    let tree_repo: Arc<dyn LocalTreeStateRepository> =
        Arc::new(SqliteLocalTreeStateRepository::new(store.clone()));
    let link_repo: Arc<dyn LocalNoteLinkRepository> =
        Arc::new(SqliteLocalNoteLinkRepository::new(store.clone()));
    let chat_session_repo: Arc<
        dyn operon_store::repos::ChatSessionRepository,
    > = Arc::new(operon_store::repos::SqliteChatSessionRepository::new(
        store.clone(),
    ));
    let chat_message_repo: Arc<
        dyn operon_store::repos::ChatMessageRepository,
    > = Arc::new(operon_store::repos::SqliteChatMessageRepository::new(
        store.clone(),
    ));
    let search_repo: Arc<dyn LocalSearchRepository> =
        Arc::new(SqliteLocalSearchRepository::new(store));
    use_context_provider(|| LocalUserRepo(user_repo));
    use_context_provider(|| LocalSettingsRepo(settings_repo));
    use_context_provider(|| LocalProjectRepo(project_repo));
    use_context_provider(|| LocalNoteRepo(note_repo));
    use_context_provider(|| LocalTreeStateRepo(tree_repo));
    use_context_provider(|| LocalNoteLinkRepo(link_repo));
    use_context_provider(|| ExplorerSearchRepo(search_repo));
    use_context_provider(|| crate::shell::companion_state::ChatSessionRepo(chat_session_repo));
    use_context_provider(|| crate::shell::companion_state::ChatMessageRepo(chat_message_repo));
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

/// Plans-Phase-5-vfs-wikilinks: wikilink graph repo. Save-time graph
/// rebuild and rename propagation read/write through this.
#[derive(Clone)]
pub struct LocalNoteLinkRepo(pub Arc<dyn LocalNoteLinkRepository>);

/// App-scope signal: gear → settings panel toggle. Lives at App scope so the
/// ActivityBar gear (rendered inside Cloud `Shell`) and the overlay can share it.
#[derive(Clone, Copy)]
pub struct SettingsOpen(pub Signal<bool>);

/// App-scope signal: latest Local username. StatusBar reads it; SettingsPanel
/// updates it on save. Seeded from `LocalUserRepo::get()`; falls back to
/// "Local user" with an upsert when the row is empty.
#[derive(Clone, Copy)]
pub struct LocalUsername(pub Signal<String>);

/// App-scope signal: currently configured notes vault root. `None` means the
/// user hasn't picked one yet (first run); `Some` is what App reads to decide
/// between mounting [`crate::local_mode::VaultDirPicker`] and the workspace.
/// SettingsPanel writes through it so a "Change…" picker hot-applies.
#[derive(Clone, Copy)]
pub struct CurrentVaultRoot(pub Signal<Option<crate::local_mode::vault::VaultRoot>>);

/// App-scope signal holding the live `LockGuard` for the currently-open
/// vault. Populated by [`crate::local_mode::VaultDirPicker`] on successful
/// pick; dropped + reacquired when the user changes the vault. Drop runs
/// at app shutdown, removing `<vault>/.operon/lock`.
#[derive(Clone, Copy)]
pub struct VaultLockHolder(pub Signal<Option<crate::local_mode::vault::LockGuard>>);

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
    let raw_note_repo: Arc<dyn LocalNoteRepository> =
        Arc::new(SqliteLocalNoteRepository::new(store.clone()));
    // Migration 018: same FS-relocating wrapper as `LocalStateProvider`
    // installs above. Renames in the UI move on-disk artifact folders.
    let vault_snapshot = crate::local_mode::vault::load(&settings_repo)
        .ok()
        .flatten();
    let note_repo: Arc<dyn LocalNoteRepository> =
        Arc::new(crate::plugins::artifact::relocate::RelocatingNoteRepo::new(
            raw_note_repo,
            vault_snapshot,
        ));
    let tree_repo: Arc<dyn LocalTreeStateRepository> =
        Arc::new(SqliteLocalTreeStateRepository::new(store.clone()));
    let link_repo: Arc<dyn LocalNoteLinkRepository> =
        Arc::new(SqliteLocalNoteLinkRepository::new(store.clone()));
    let chat_session_repo: Arc<
        dyn operon_store::repos::ChatSessionRepository,
    > = Arc::new(operon_store::repos::SqliteChatSessionRepository::new(
        store.clone(),
    ));
    let chat_message_repo: Arc<
        dyn operon_store::repos::ChatMessageRepository,
    > = Arc::new(operon_store::repos::SqliteChatMessageRepository::new(
        store.clone(),
    ));
    let search_repo: Arc<dyn LocalSearchRepository> =
        Arc::new(SqliteLocalSearchRepository::new(store));
    use_context_provider(|| LocalUserRepo(user_repo));
    use_context_provider(|| LocalSettingsRepo(settings_repo));
    use_context_provider(|| LocalProjectRepo(project_repo));
    use_context_provider(|| LocalNoteRepo(note_repo));
    use_context_provider(|| LocalTreeStateRepo(tree_repo));
    use_context_provider(|| LocalNoteLinkRepo(link_repo));
    use_context_provider(|| ExplorerSearchRepo(search_repo));
    use_context_provider(|| crate::shell::companion_state::ChatSessionRepo(chat_session_repo));
    use_context_provider(|| crate::shell::companion_state::ChatMessageRepo(chat_message_repo));
}

fn open_local_store() -> Store {
    let path = default_store_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            panic!(
                "operon: cannot create local store directory {parent:?}: {e}\n\
                 Persistence is unavailable. Fix the directory's permissions (or set $HOME) \
                 and restart."
            );
        }
    }
    // Previously this fell back to `:memory:` on failure, which silently
    // discarded every settings write across sessions (mode_remembered,
    // vault.root.path, …). Persistence-by-RAM was indistinguishable from
    // a fresh install on every restart. Now we panic loudly with the
    // underlying error so the failure mode is at least debuggable.
    match Store::open(StoreConfig::local(&path)) {
        Ok(store) => store,
        Err(e) => panic!(
            "operon: failed to open local store at {path:?}: {e}\n\
             Persistence is unavailable. Common causes:\n\
               - The file was created by a newer build that applied a migration\n\
                 this binary doesn't know about. Inspect _schema_migrations.\n\
               - Disk permissions on {path:?}.\n\
               - Corrupt SQLite file. Removing it restores a fresh install."
        ),
    }
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

#[component]
pub fn SettingsPanel(open: Signal<bool>, username: Signal<String>) -> Element {
    let LocalUserRepo(user_repo) = use_context();
    let CurrentVaultRoot(mut vault_root) = use_context();
    let mut draft: Signal<String> = use_signal(|| username.read().clone());
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut show_change_picker: Signal<bool> = use_signal(|| false);

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

    let vault_path_label = vault_root
        .read()
        .as_ref()
        .map(|r| r.path.display().to_string())
        .unwrap_or_else(|| "(not set)".to_string());

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "settings-panel",
            onclick: move |_| close(),
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    close();
                    evt.prevent_default();
                }
            },
            div {
                class: "operon-modal-card",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title", "Local user" }
                label { class: "operon-modal-label", "Username" }
                input {
                    r#type: "text",
                    class: "operon-modal-input",
                    "data-testid": "username-input",
                    value: "{draft.read()}",
                    autofocus: true,
                    oninput: move |evt| draft.set(evt.value()),
                }
                if let Some(msg) = error.read().clone() {
                    p { class: "operon-modal-error", "{msg}" }
                }
                h3 {
                    class: "operon-modal-section",
                    style: "margin-top: 1rem; font-weight: 600;",
                    "Vault directory"
                }
                div {
                    class: "operon-modal-vault-row",
                    style: "display: flex; align-items: center; gap: 0.5rem;",
                    code {
                        "data-testid": "vault-path",
                        style: "flex: 1; padding: 0.25rem 0.5rem; background: var(--operon-bg-2, #f5f5f5); border-radius: 0.25rem; font-size: 0.85em;",
                        "{vault_path_label}"
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        "data-testid": "vault-change-button",
                        onclick: move |_| show_change_picker.set(true),
                        "Change…"
                    }
                }
                p {
                    class: "operon-modal-help",
                    style: "font-size: 0.8em; color: var(--operon-fg-muted, #666); margin-top: 0.25rem;",
                    "Changing the vault re-points new writes; existing notes stay in their previous location."
                }
                // Slice A4b: provider API keys. The section reads
                // `SettingsServiceCtx` from context so we don't have to
                // pass a non-PartialEq prop through Dioxus.
                crate::shell::settings::ProvidersSection {}
                // Global Claude defaults — bottom tier of the
                // three-tier hierarchy (chat → project → global).
                // Per-project tool-permissions auto-approve toggles
                // live on the project row itself (gear icon + context
                // menu → Tool permissions…) — they're per-repo and
                // belong on the project, not in global Settings.
                crate::shell::settings::ClaudeDefaultsSection {}
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button",
                        onclick: move |_| close(),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        onclick: move |_| save(),
                        "Save"
                    }
                }
            }
            if *show_change_picker.read() {
                crate::local_mode::VaultDirPicker {
                    blocking: false,
                    on_chosen: move |root: crate::local_mode::vault::VaultRoot| {
                        // Wipe any leftover Ctrl+Z trash in the new vault;
                        // trash is session-scoped and shouldn't survive a
                        // vault switch.
                        crate::plugins::cleanup::trash::wipe_trash_root(
                            &crate::plugins::cleanup::trash::vault_trash_root(root.path()),
                        );
                        vault_root.set(Some(root));
                        show_change_picker.set(false);
                    },
                }
            }
        }
    }
}

/// Single-button chooser shown on first launch (or whenever
/// `local_app_settings.mode_remembered` is empty).
///
/// Previously offered Local + Cloud (RBAG). Cloud was removed per
/// product direction — only Local remains. Once the user picks Local
/// the setting persists in `local_app_settings.mode_remembered`, so
/// subsequent launches skip the chooser entirely.
///
/// `MODE_VALUE_CLOUD` is preserved as a constant so any historical
/// DB row carrying it still round-trips through `read_remembered_mode`
/// without panicking — but no UI path produces a new one.
#[component]
pub fn StartupChooser() -> Element {
    let LocalSettingsRepo(settings_repo) = use_context();
    let mut state = use_context::<Signal<AppState>>();
    let crate::local_mode::ModeChosen(mut mode_chosen) = use_context();

    let pick_local = {
        let settings = settings_repo.clone();
        move |_| {
            let _ = settings.set(SETTINGS_KEY_MODE_REMEMBERED, MODE_VALUE_LOCAL);
            state.with_mut(|s| s.mode = Mode::Local);
            mode_chosen.set(true);
        }
    };

    rsx! {
        div {
            class: "flex flex-col items-center justify-center h-screen w-screen gap-6 bg-[var(--operon-bg)] text-[var(--operon-fg)]",
            "data-testid": "mode-chooser",
            h1 { class: "text-lg font-semibold", "Welcome to Operon" }
            p { class: "text-sm opacity-70 max-w-md text-center",
                "Operon runs against a local notes vault on this machine. Click below to continue."
            }
            button {
                r#type: "button",
                class: "px-8 py-6 rounded-md border border-[var(--operon-border)] hover:bg-[var(--operon-hover)] text-base font-medium",
                "data-testid": "chooser-local",
                onclick: pick_local,
                "Start"
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

/// Read the persisted vault root path. Used by `app.rs` in Local Mode to
/// decide whether to render the [`VaultDirPicker`] or jump straight into the
/// workspace. Returns `None` when no vault has been picked yet (first run)
/// OR when the stored path no longer points at an existing directory — so
/// a moved/deleted vault re-prompts on next launch instead of sailing into
/// the workspace and failing every downstream write.
pub fn read_vault_root(
    settings: &Arc<dyn LocalSettingsRepository>,
) -> Option<crate::local_mode::vault::VaultRoot> {
    let stored = crate::local_mode::vault::load(settings).ok().flatten()?;
    if stored.path.is_dir() {
        Some(stored)
    } else {
        eprintln!(
            "operon: stored vault {:?} no longer exists or is not a directory; \
             re-prompting via VaultDirPicker.",
            stored.path
        );
        None
    }
}

/// note_id -> set of body hashes we ourselves wrote and expect notify
/// to echo back. Inserted just before `persistence.save` and removed
/// on the first matching watcher sweep so our own augmented save
/// doesn't keep retriggering the diff/append path and grow the
/// revision table unboundedly.
type PendingSelfWrites = Arc<AsyncMutex<HashMap<String, HashSet<u64>>>>;

fn hash_body(body: &str) -> u64 {
    let mut h = DefaultHasher::new();
    body.as_bytes().hash(&mut h);
    h.finish()
}

/// Walk every open tab, reload its body via `persistence.load(note_id)`,
/// and write back via `TabManager::reload_content` only when the content
/// actually changed. Used by the artifact watcher to round-trip
/// `<repo>/.operon/artifacts/.../index.md` writes back into the editor
/// without requiring the file-event to identify a specific note id
/// (artifact paths are slug-based, not uuid-based).
///
/// Skips no-op writes so a re-render isn't triggered when content
/// matches. Bumps `LOCAL_NOTE_VERSION` once at the end if anything
/// changed so explorer rows etc. re-render.
async fn reload_open_tabs_from_disk(
    persistence: Arc<dyn Persistence>,
    tabs: Signal<TabManager>,
    note_repo: Arc<dyn LocalNoteRepository>,
    pending: PendingSelfWrites,
) {
    let tab_snapshot: Vec<(TabId, String)> = tabs
        .read()
        .iter()
        .map(|t| (t.id, t.note_id.clone()))
        .collect();
    let mut updated_any = false;
    for (tid, note_id_str) in tab_snapshot {
        let bytes = match persistence.load(&note_id_str).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let id = match Uuid::parse_str(&note_id_str) {
            Ok(u) => u,
            Err(_) => continue,
        };
        // Echo check: if the disk content matches a body we ourselves
        // just wrote (via the augmented-save path below), drop the
        // pending entry, silently sync the tab, and skip diff/append.
        // Without this, our own atomic temp+rename save re-triggers
        // notify, the prior_body == disk_body short-circuit races
        // against the rename event ordering, and the revision table
        // grows by one row per echo forever.
        let disk_hash = hash_body(&body);
        {
            let mut pending_lock = pending.lock().await;
            if let Some(set) = pending_lock.get_mut(&note_id_str) {
                if set.remove(&disk_hash) {
                    if set.is_empty() {
                        pending_lock.remove(&note_id_str);
                    }
                    drop(pending_lock);
                    let mut tabs_sig = tabs;
                    let mut tabs_w = tabs_sig.write();
                    tabs_w.reload_content(tid, body);
                    updated_any = true;
                    continue;
                }
            }
        }
        // Snapshot the tab's pre-reload body so we can decide whether
        // to short-circuit AND so the artifact branch below can
        // summarise the delta.
        let prior_body = tabs.read().get(tid).map(|t| t.content.clone());
        if prior_body.as_deref() == Some(body.as_str()) {
            continue;
        }
        // Is this note an Artifact? Only artifacts get the in-body
        // revision-history table. Non-artifacts: silent reload.
        let kind = note_repo
            .find_project_for_note(id)
            .ok()
            .flatten()
            .and_then(|pid| note_repo.list_for_project(pid).ok())
            .and_then(|notes| notes.into_iter().find(|n| n.id == id).map(|n| n.kind));
        let is_artifact = matches!(kind, Some(NoteKind::Artifact));

        if is_artifact {
            // PostToolUse hook is wired up: it owns the revision-row
            // append and ships Claude's actual explanation as the
            // summary. We just sync the tab buffer to disk here so
            // the user sees the new content immediately; the hook's
            // subsequent save lands the row a moment later.
            if crate::local_mode::reload_socket::is_bound() {
                let mut tabs_sig = tabs;
                let mut tabs_w = tabs_sig.write();
                tabs_w.reload_content(tid, body);
                updated_any = true;
                continue;
            }
            // Hook unavailable — keep the original behaviour: build
            // a `claude` revision row from the disk-vs-tab diff and
            // stitch it into the body BEFORE writing back. Order of
            // operations: append → set tab content → save. By the
            // time `persistence.save` re-fires the notify watcher,
            // `tab.content` already equals `body_with_row`, so the
            // next pass sees prior == disk and short-circuits.
            let summary = revision_table::compute_summary(
                prior_body.as_deref(),
                Some(body.as_str()),
            );
            let date = revision_table::format_revision_date(now_unix_ms());
            let row = revision_table::RevisionRow {
                revision: revision_table::next_revision_number(&body),
                date,
                derived_from: "claude".to_string(),
                summary,
            };
            let body_with_row = revision_table::append_revision_row(&body, row);
            let mut tabs_sig = tabs;
            {
                let mut tabs_w = tabs_sig.write();
                tabs_w.reload_content(tid, body_with_row.clone());
            }
            // Record the expected echo before we hit disk so the
            // watcher's next sweep can recognise it as our own write
            // rather than treating it as another external edit.
            {
                let mut pending_lock = pending.lock().await;
                pending_lock
                    .entry(note_id_str.clone())
                    .or_default()
                    .insert(hash_body(&body_with_row));
            }
            if let Err(e) = persistence
                .save(&note_id_str, body_with_row.as_bytes())
                .await
            {
                tracing::warn!(
                    target: "operon::revision",
                    "watcher write-back of revision row for {id} failed: {e}"
                );
            }
            updated_any = true;
        } else {
            let mut tabs_sig = tabs;
            let mut tabs_w = tabs_sig.write();
            tabs_w.reload_content(tid, body);
            updated_any = true;
        }
    }
    if updated_any {
        *crate::shell::companion_state::LOCAL_NOTE_VERSION.write() += 1;
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Reload the open tab whose persistence-resolved on-disk path matches
/// `incoming` from disk, and for artifact tabs append a revision row
/// using `summary` (or a diff-based fallback when `summary` is None).
/// Used by the PostToolUse hook bridge: `operon-posttool-hook` reports
/// the absolute path Claude just wrote plus Claude's preceding
/// explanation, and we walk every open tab to find the one that maps
/// to it.
///
/// Path comparison canonicalizes both sides where possible (so a
/// symlinked vault or a path with `..` components still matches);
/// falls back to lexical equality when canonicalize fails (the most
/// common reason: the target was just deleted, in which case we
/// skip — there's nothing to reload).
///
/// When the matched tab is an Artifact, this function takes ownership
/// of the revision-row append (the inotify watcher defers to us via
/// [`reload_socket::is_bound`]). The save's hash is recorded in the
/// shared `PendingSelfWrites` map so the watcher's next sweep silently
/// reloads instead of stacking another row.
async fn reload_open_tab_by_path(
    persistence: Arc<dyn Persistence>,
    tabs: Signal<TabManager>,
    note_repo: Arc<dyn LocalNoteRepository>,
    pending: PendingSelfWrites,
    incoming: PathBuf,
    summary: Option<String>,
) {
    let incoming_canon = std::fs::canonicalize(&incoming).unwrap_or(incoming);
    let tab_snapshot: Vec<(TabId, String)> = tabs
        .read()
        .iter()
        .map(|t| (t.id, t.note_id.clone()))
        .collect();
    for (tid, note_id_str) in tab_snapshot {
        let Some(resolved) = persistence.resolved_path(&note_id_str) else {
            continue;
        };
        let resolved_canon = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        if resolved_canon != incoming_canon {
            continue;
        }
        let bytes = match persistence.load(&note_id_str).await {
            Ok(b) => b,
            Err(_) => return,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return,
        };
        let id = match Uuid::parse_str(&note_id_str) {
            Ok(u) => u,
            Err(_) => return,
        };
        let prior_body = tabs.read().get(tid).map(|t| t.content.clone());
        // Artifact branch: build + persist a revision row whose
        // summary is Claude's preceding explanation when we have it,
        // falling back to the diff-based summary used by the watcher
        // when the hook couldn't extract a transcript text block.
        let kind = note_repo
            .find_project_for_note(id)
            .ok()
            .flatten()
            .and_then(|pid| note_repo.list_for_project(pid).ok())
            .and_then(|notes| notes.into_iter().find(|n| n.id == id).map(|n| n.kind));
        let is_artifact = matches!(kind, Some(NoteKind::Artifact));

        if is_artifact {
            // Skip if disk + tab + (no new summary) all agree —
            // re-saves with no real change would otherwise stack a
            // row per identical write.
            if prior_body.as_deref() == Some(body.as_str()) && summary.is_none() {
                return;
            }
            let summary_text = summary.unwrap_or_else(|| {
                revision_table::compute_summary(prior_body.as_deref(), Some(body.as_str()))
            });
            let date = revision_table::format_revision_date(now_unix_ms());
            let row = revision_table::RevisionRow {
                revision: revision_table::next_revision_number(&body),
                date,
                derived_from: "claude".to_string(),
                summary: summary_text,
            };
            let body_with_row = revision_table::append_revision_row(&body, row);
            let mut tabs_sig = tabs;
            {
                let mut tabs_w = tabs_sig.write();
                tabs_w.reload_content(tid, body_with_row.clone());
            }
            {
                let mut pending_lock = pending.lock().await;
                pending_lock
                    .entry(note_id_str.clone())
                    .or_default()
                    .insert(hash_body(&body_with_row));
            }
            if let Err(e) = persistence
                .save(&note_id_str, body_with_row.as_bytes())
                .await
            {
                tracing::warn!(
                    target: "operon::revision",
                    "hook write-back of revision row for {id} failed: {e}"
                );
            }
        } else {
            // Non-artifact: silent reload, matches the watcher path.
            if prior_body.as_deref() == Some(body.as_str()) {
                return;
            }
            let mut tabs_sig = tabs;
            let mut tabs_w = tabs_sig.write();
            tabs_w.reload_content(tid, body);
        }
        *crate::shell::companion_state::LOCAL_NOTE_VERSION.write() += 1;
        return;
    }
}

/// Lift every Local-Mode app-scope signal to App scope so the Cloud `Shell`
/// chrome (mode-aware StatusBar / ActivityBar / SideBar plugin contributions)
/// can read them without prop-drilling. Call from `app.rs` only when
/// `Mode::Local`, after `provide_local_state()`.
pub fn provide_local_app_signals() {
    let LocalUserRepo(user_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo_for_app_signals) = use_context();
    let CurrentVaultRoot(vault_root_for_trash_wipe) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let tabs: Signal<TabManager> = use_context();
    // Same vault signal as the trash-wipe block, but used by the
    // filesystem watcher below to (re)build a watcher whenever the
    // user changes vaults.
    let vault_root_for_watcher = vault_root_for_trash_wipe;

    // Session start: wipe any leftover trash from a previous run.
    // Per-project artifact trash lives at
    // `<vault>/.operon/<project-id>/trash/`; vault-wide blob trash at
    // `<vault>/.operon/trash/`; per-repo skill trash at
    // `<repo>/.claude/trash/`. Trash is session-scoped — Ctrl+Z within
    // a session restores; nothing persists across restarts.
    {
        let project_repo = project_repo_for_app_signals.clone();
        let vault_root = vault_root_for_trash_wipe;
        use_hook(move || {
            use crate::plugins::cleanup::trash;
            let vault_snapshot = vault_root.peek().clone();
            if let Ok(projects) = project_repo.list() {
                for p in projects {
                    if let Some(repo) = p.repo_path.as_ref() {
                        trash::wipe_trash_root(&trash::repo_skill_trash_root(repo));
                    }
                    if let Some(ref vault) = vault_snapshot {
                        trash::wipe_trash_root(&trash::project_trash_root(vault, p.id));
                    }
                }
            }
            if let Some(vault) = vault_snapshot {
                trash::wipe_trash_root(&trash::vault_trash_root(vault.path()));
            }
        });
    }

    // Filesystem watcher #1 — `<vault>/notes/` for non-artifact notes
    // (opaque UUID-named files). Detect external writes (typically
    // Claude's `Write`/`Edit` tool against a referenced note) and
    // reload the matching open editor tab so the user sees the new
    // content without re-opening the file. Auto-reload is silent: if
    // the user has unsaved local edits they're overwritten by disk
    // content (matches the system-prompt promise to Claude that "the
    // app watches that directory and will pick up your changes
    // automatically"). Body only — SQLite metadata (titles, parent
    // IDs) isn't touched here.
    //
    // Watcher #2 is built further down for each project's
    // `<repo>/.operon/artifacts/` (recursive) so artifact-kind notes
    // round-trip too.
    //
    // The handle is held in a `Rc<RefCell<Option<(PathBuf, _)>>>` so we
    // can compare against the current vault's `notes_dir` and skip
    // rebuilds when nothing changed, and drop the previous handle when
    // the user switches vaults. notify's callback runs on its own OS
    // thread; we bridge to the UI through an `unbounded_channel` whose
    // receiver task is spawned via `dioxus::spawn` (so it owns the
    // runtime context required to write to component-scope signals).
    {
        let watch_holder: Rc<RefCell<Option<(std::path::PathBuf, WatchHandle)>>> =
            use_hook(|| Rc::new(RefCell::new(None)));
        let vault_signal = vault_root_for_watcher;
        let persistence_for_watcher = persistence.clone();
        let tabs_for_watcher = tabs;
        use_effect(move || {
            let Some(notes_dir) = vault_signal.read().as_ref().map(|v| v.notes_dir()) else {
                return;
            };
            // Skip when we're already watching this directory.
            if let Some((current_dir, _)) = watch_holder.borrow().as_ref() {
                if current_dir == &notes_dir {
                    return;
                }
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Uuid>();
            let watcher = FilesystemWatcher::new(notes_dir.clone());
            let handle = watcher.subscribe(Box::new(move |evt: WatchEvent| {
                let name = match evt {
                    WatchEvent::Modified(n) | WatchEvent::Created(n) => n,
                    _ => return,
                };
                if let Ok(uuid) = Uuid::parse_str(&name) {
                    let _ = tx.send(uuid);
                }
            }));

            let persistence_for_task = persistence_for_watcher.clone();
            let tabs_for_task = tabs_for_watcher;
            spawn(async move {
                while let Some(uuid) = rx.recv().await {
                    let note_id_str = uuid.to_string();
                    let bytes = match persistence_for_task.load(&note_id_str).await {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let body = match String::from_utf8(bytes) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let mut tabs_sig = tabs_for_task;
                    let mut updated = false;
                    {
                        let mut tabs_w = tabs_sig.write();
                        let tab_ids: Vec<TabId> = tabs_w
                            .iter()
                            .filter(|t| t.note_id == note_id_str && t.content != body)
                            .map(|t| t.id)
                            .collect();
                        if !tab_ids.is_empty() {
                            updated = true;
                            for tid in tab_ids {
                                tabs_w.reload_content(tid, body.clone());
                            }
                        }
                    }
                    if updated {
                        // Non-artifact notes don't carry a revision
                        // table — the cascade runner's `<details>`
                        // history covers what little auditing is
                        // needed elsewhere.
                        *crate::shell::companion_state::LOCAL_NOTE_VERSION.write() += 1;
                    }
                }
            });

            *watch_holder.borrow_mut() = Some((notes_dir, handle));
        });
    }

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

    // Filesystem watcher #2 — each project's
    // `<vault>/.operon/<project-id>/artifacts/` directory. Artifact
    // notes don't live in the vault's opaque UUID store; their bodies
    // are written under the per-project artifacts dir by
    // `ArtifactPersistence::save` (see `VaultRoot::project_artifacts_dir`).
    // The filename is `index.md`, not a UUID, so the watcher can't
    // identify a specific note from the event — instead we reload
    // every open tab's body from `persistence` on any change.
    // `persistence.load` routes through `ArtifactPersistence` which
    // knows the correct path per note id; `reload_content` skips
    // tabs whose buffer already matches, so the broad reload is
    // mostly idempotent.
    //
    // Re-runs when `project_version` bumps (so newly-added projects
    // get watched and removed projects' watchers go away) AND when
    // the vault root changes (so switching vaults retargets at the
    // new on-disk layout instead of stale paths from the previous
    // vault).
    {
        let watch_holder: Rc<RefCell<Vec<(PathBuf, WatchHandle)>>> =
            use_hook(|| Rc::new(RefCell::new(Vec::new())));
        let project_repo_for_watcher = project_repo_for_app_signals.clone();
        let persistence_for_artifact_watcher = persistence.clone();
        let tabs_for_artifact_watcher = tabs;
        let project_version_for_watcher: Signal<u64> = project_version;
        let vault_root_for_artifact_watcher = vault_root_for_watcher;
        // The artifact watcher's reload sweep needs the note repo to
        // distinguish artifacts (which get the in-body revision row
        // append) from other kinds. Snapshot once per effect run.
        let LocalNoteRepo(note_repo_for_artifact_watcher) = use_context::<LocalNoteRepo>();
        use_effect(move || {
            // Subscribe to project_version + vault_root so the effect
            // re-runs on create/rename/remove of projects AND on vault
            // switch.
            let _ = project_version_for_watcher.read();
            let vault_snapshot = vault_root_for_artifact_watcher.read().clone();
            let Some(vault) = vault_snapshot else {
                // No vault bound yet — tear down anything we were
                // watching (a previous vault's dirs) and bail. The
                // next vault-set will re-fire this effect.
                watch_holder.borrow_mut().clear();
                return;
            };

            let projects = match project_repo_for_watcher.list() {
                Ok(p) => p,
                Err(_) => return,
            };
            let mut target_dirs: Vec<PathBuf> = Vec::new();
            for p in &projects {
                target_dirs.push(vault.project_artifacts_dir(p.id));
            }
            // Skip when the watcher set is already pointing at the same
            // paths (cheap PartialEq on Vec<PathBuf>).
            {
                let current: Vec<PathBuf> = watch_holder
                    .borrow()
                    .iter()
                    .map(|(p, _)| p.clone())
                    .collect();
                if current == target_dirs {
                    return;
                }
            }
            // Tear down the previous set and rebuild. Dropping each
            // `WatchHandle` stops the underlying notify watcher.
            watch_holder.borrow_mut().clear();

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            for dir in &target_dirs {
                // Create the dir if missing so notify can attach. Tiny
                // cost on each effect run; tolerates pre-existing dirs.
                if let Err(e) = std::fs::create_dir_all(dir) {
                    tracing::warn!(
                        target: "operon::watcher",
                        "create artifacts dir {dir:?}: {e}"
                    );
                    continue;
                }
                // Recursive notify watcher inline — the existing
                // `FilesystemWatcher` helper is NonRecursive + emits
                // basenames only, both wrong for artifact paths.
                let tx_for_dir = tx.clone();
                let watch_result = (|| -> Result<WatchHandle, ()> {
                    use notify::{recommended_watcher, EventKind, RecursiveMode, Watcher};
                    let mut watcher = recommended_watcher(
                        move |res: notify::Result<notify::Event>| {
                            let Ok(ev) = res else { return };
                            if !matches!(
                                ev.kind,
                                EventKind::Create(_) | EventKind::Modify(_)
                            ) {
                                return;
                            }
                            let _ = tx_for_dir.send(());
                        },
                    )
                    .map_err(|_| ())?;
                    watcher
                        .watch(dir.as_path(), RecursiveMode::Recursive)
                        .map_err(|_| ())?;
                    Ok(WatchHandle {
                        _inner: Some(Box::new(watcher)),
                    })
                })();
                match watch_result {
                    Ok(handle) => {
                        watch_holder.borrow_mut().push((dir.clone(), handle));
                    }
                    Err(_) => {
                        tracing::warn!(
                            target: "operon::watcher",
                            "failed to attach artifact watcher to {dir:?}"
                        );
                    }
                }
            }
            // Single reader task: any event from any artifact dir
            // triggers a full-tab reload sweep. Coalesce bursts via an
            // inner `try_recv` drain so a Write tool's tempfile +
            // rename (2-3 events) becomes one sweep.
            let persistence_for_task = persistence_for_artifact_watcher.clone();
            let tabs_for_task = tabs_for_artifact_watcher;
            let note_repo_for_task = note_repo_for_artifact_watcher.clone();
            let pending_for_task: PendingSelfWrites =
                Arc::new(AsyncMutex::new(HashMap::new()));
            spawn(async move {
                while rx.recv().await.is_some() {
                    while rx.try_recv().is_ok() {}
                    reload_open_tabs_from_disk(
                        persistence_for_task.clone(),
                        tabs_for_task,
                        note_repo_for_task.clone(),
                        pending_for_task.clone(),
                    )
                    .await;
                }
            });
        });
    }
    // Bridge: detached-scope writers (artifact `spawn_forever` cascade,
    // workflow executor) bump `LOCAL_NOTE_VERSION` (a `GlobalSignal`)
    // because writes from the virtual root scope to a component-scope
    // `Signal` get the `__copy_value_hoisted` warning and may be
    // silently dropped. This effect mirrors those bumps back into the
    // local Signal so existing component readers (the explorer's
    // `notes_by_project` Memo, etc.) re-render without per-call-site
    // migration. One unified subscriber here, no churn at the dozens
    // of read sites scattered across the codebase.
    {
        let mut local = note_version;
        use_effect(move || {
            let _v = *crate::shell::companion_state::LOCAL_NOTE_VERSION.read();
            local.with_mut(|v| *v = v.saturating_add(1));
        });
    }

    // PostToolUse reload bridge: Claude's `operon-posttool-hook` posts
    // `{tool,path,summary}` over the Unix socket bound by
    // `reload_socket::start()`. We walk every open tab, ask its
    // persistence layer for the on-disk path, and reload the matching
    // tab from disk — and for artifacts, append a revision row using
    // Claude's preceding assistant text as the summary so the table
    // says "Updated criterion to 8x8 round" instead of the diff-based
    // "Edited body (61 lines)".
    //
    // Deterministic backstop for the inotify watchers (#1 and #2):
    // notify can drop events on NFS / encrypted overlays / atomic-
    // rename sequences, but Claude itself tells us exactly which
    // file it just wrote.
    {
        let persistence_for_hook = persistence.clone();
        let tabs_for_hook = tabs;
        let LocalNoteRepo(note_repo_for_hook) = use_context::<LocalNoteRepo>();
        // Reuse the artifact watcher's self-write echo map so saves
        // we initiate from the hook path are recognised by the
        // watcher's next sweep — no duplicate revision rows.
        let pending_for_hook: PendingSelfWrites =
            use_hook(|| Arc::new(AsyncMutex::new(HashMap::new())));
        use_hook(move || {
            let persistence = persistence_for_hook.clone();
            let tabs_sig = tabs_for_hook;
            let note_repo = note_repo_for_hook.clone();
            let pending = pending_for_hook.clone();
            spawn(async move {
                let Some(mut rx) =
                    crate::local_mode::reload_socket::take_receiver().await
                else {
                    return;
                };
                while let Some(evt) = rx.recv().await {
                    reload_open_tab_by_path(
                        persistence.clone(),
                        tabs_sig,
                        note_repo.clone(),
                        pending.clone(),
                        evt.path,
                        evt.summary,
                    )
                    .await;
                }
            });
        });
    }

    // TOC auto-refresh: any time `LOCAL_NOTE_VERSION` bumps (rename,
    // create, delete, reparent, …), walk every open tab whose body
    // carries the `<!-- operon:toc -->` sentinel and regenerate the
    // Contents block against the current note tree. The on-open
    // refresh path in the explorer already covers the first render;
    // this covers everything after — so a child rename surfaces in
    // the parent's Contents without the user having to close and
    // reopen the tab.
    //
    // Loop avoidance: we update the tab buffer BEFORE writing back to
    // disk. The filesystem watcher's identity check (`disk_body ==
    // tab.content`) then short-circuits the watcher's reload sweep,
    // so the artifact-row auto-append path in the watcher doesn't
    // also fire from our save.
    {
        let persistence_for_toc = persistence.clone();
        let tabs_for_toc = tabs;
        let LocalNoteRepo(note_repo_for_toc) = use_context::<LocalNoteRepo>();
        use_effect(move || {
            // Subscribe to the bump.
            let _v = *crate::shell::companion_state::LOCAL_NOTE_VERSION.read();
            // Snapshot every open tab's id + note_id + body so the
            // async sweep below can release the read lock immediately.
            let tab_snapshot: Vec<(TabId, String, String)> = tabs_for_toc
                .read()
                .iter()
                .map(|t| (t.id, t.note_id.clone(), t.content.clone()))
                .collect();
            let pers = persistence_for_toc.clone();
            let nr = note_repo_for_toc.clone();
            let tabs_sig = tabs_for_toc;
            spawn(async move {
                for (tid, note_id_str, body) in tab_snapshot {
                    if !body.contains(crate::plugins::toc::TOC_SENTINEL) {
                        continue;
                    }
                    let Ok(uuid) = Uuid::parse_str(&note_id_str) else {
                        continue;
                    };
                    let project_id = match nr.find_project_for_note(uuid) {
                        Ok(Some(p)) => p,
                        _ => continue,
                    };
                    let notes = match nr.list_for_project(project_id) {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    let refreshed = crate::plugins::toc::refresh_if_managed(
                        &body, uuid, &notes,
                    );
                    if refreshed == body {
                        continue;
                    }
                    // Update tab buffer first so the next filesystem
                    // watcher tick sees disk == buffer and skips.
                    {
                        let mut tabs_sig = tabs_sig;
                        let mut tabs_w = tabs_sig.write();
                        tabs_w.reload_content(tid, refreshed.clone());
                    }
                    if let Err(e) = pers.save(&note_id_str, refreshed.as_bytes()).await {
                        tracing::warn!(
                            target: "operon::toc",
                            "auto-refresh save({note_id_str}): {e}"
                        );
                    }
                }
            });
        });
    }
    let selected_project: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| SelectedProject(selected_project));
    // M1-companion-claude-code: expose the active project's repo_path as a
    // shared signal so the companion-pane Claude session can rebind its cwd.
    let active_repo_path: Signal<Option<std::path::PathBuf>> = use_signal(|| None);
    use_context_provider(|| {
        crate::shell::companion_state::ActiveRepoPath(active_repo_path)
    });
    // M1.5a-multi-session: companion-pane scope tab + currently-open chat
    // session. Default scope: Vault (companion can chat against the vault
    // even before any project is selected). The session rail flips this
    // when the user clicks the Project / Global tab.
    let active_chat_scope: Signal<crate::shell::companion_state::ChatScope> =
        use_signal(|| crate::shell::companion_state::ChatScope::Vault);
    use_context_provider(|| {
        crate::shell::companion_state::ActiveChatScope(active_chat_scope)
    });
    let active_chat_session: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| {
        crate::shell::companion_state::ActiveChatSession(active_chat_session)
    });
    let chat_session_version: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| {
        crate::shell::companion_state::ChatSessionVersion(chat_session_version)
    });
    // NOTE: `ChatMessageVersion` is now provided in `app.rs::App()`
    // (root scope) instead of here. The artifact runner's
    // `spawn_forever` task lives at root, and Dioxus signals can
    // only be written from within their owning scope's subtree —
    // creating it in Workspace meant root-scoped writes were
    // silently dropped (Dioxus logs `__copy_value_hoisted` warnings).
    let companion_composer_inbox: Signal<Option<String>> = use_signal(|| None);
    use_context_provider(|| {
        crate::shell::companion_state::CompanionComposerInbox(companion_composer_inbox)
    });
    // Append-semantics sibling of the inbox above. The side-bar's
    // "Send to chat" right-click writes a `@[<title>](note:<uuid>)`
    // mention token here; the companion appends it to the composer
    // without clobbering the user's draft.
    let companion_composer_append: Signal<Option<String>> = use_signal(|| None);
    use_context_provider(|| {
        crate::shell::companion_state::CompanionComposerAppend(companion_composer_append)
    });
    // M3c: shared Claude Code plugin instance. Companion + workflow
    // executor both consume this so a workflow cascade reuses claude
    // session caching without spawning duplicate subprocesses.
    let claude_plugin = use_hook(|| {
        std::sync::Arc::new(
            operon_plugins_claude_code::ClaudeCodeChatPlugin::new(
                operon_plugins_claude_code::ClaudeCodeConfig {
                    claude_bin: crate::shell::companion_chat::resolve_claude_bin(),
                    model: None,
                    shim_bin: crate::shell::companion_chat::resolve_mcp_permission_shim(),
                },
            ),
        )
    });
    // Hydrate the plugin's global defaults from local_app_settings so the
    // model + permission-mode dropdowns survive app restarts, project
    // switches, and new chat sessions.
    {
        let LocalSettingsRepo(settings_repo) = use_context();
        let claude_plugin = claude_plugin.clone();
        use_hook(move || {
            if let Ok(Some(m)) =
                settings_repo.get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_MODEL)
            {
                if !m.is_empty() {
                    claude_plugin.set_default_model(Some(m));
                }
            }
            if let Ok(Some(pm)) = settings_repo
                .get(crate::local_mode::SETTINGS_KEY_CLAUDE_DEFAULT_PERMISSION_MODE)
            {
                if !pm.is_empty() {
                    claude_plugin.set_permission_mode(Some(pm));
                }
            }
        });
    }
    use_context_provider(|| {
        crate::shell::companion_state::ClaudeCodePluginCtx(claude_plugin.clone())
    });

    // Slice A14 cutover: build the in-process runtime backend alongside
    // the legacy subprocess plugin. Both are made available via
    // `BackendsCtx`; `AgentBackendCtx` (a Signal) tracks the user's
    // current choice. Default = ClaudeCode for safety.
    let (backends, settings_service) =
        use_hook(|| build_backends_and_settings(claude_plugin.clone()));
    use_context_provider(|| backends.clone());
    let active_backend: Signal<std::sync::Arc<dyn operon_core::agent_event::AgentBackend>> =
        use_signal(|| backends.claude_code.clone());
    use_context_provider(|| crate::shell::companion_state::AgentBackendCtx(active_backend));
    // Settings service (Slice A4b) shares the same `LayeredSecretStore` as
    // the runtime backend's factory so a key written from the settings
    // page takes effect on the next runtime turn without an app restart.
    use_context_provider(|| {
        crate::shell::settings::SettingsServiceCtx(settings_service.clone())
    });
    // MCP settings service — wraps the `claude mcp ...` CLI. The cwd is
    // initialised to None and re-resolved per call against the active
    // repo path; we don't bake it in here because the panel is shared
    // across project switches.
    use_context_provider(|| {
        let claude_bin = crate::shell::companion_chat::resolve_claude_bin();
        crate::shell::mcp_settings::McpServiceCtx(std::sync::Arc::new(
            crate::shell::mcp_settings::McpService::new(claude_bin),
        ))
    });
    let selected_note: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| SelectedNote(selected_note));
    // Plans-Phase-4-multiselect-aria: parallel multi-selection set.
    let multi_selected: Signal<std::collections::BTreeSet<crate::local_mode::explorer::NodeKey>> =
        use_signal(std::collections::BTreeSet::new);
    use_context_provider(|| crate::local_mode::explorer::MultiSelected(multi_selected));
    let last_clicked: Signal<Option<crate::local_mode::explorer::NodeKey>> = use_signal(|| None);
    use_context_provider(|| crate::local_mode::explorer::LastClicked(last_clicked));
    let focused_node: Signal<Option<crate::local_mode::explorer::NodeKey>> =
        use_signal(|| None);
    use_context_provider(|| crate::local_mode::explorer::FocusedNode(focused_node));
    let visible_flat: Signal<Vec<crate::local_mode::explorer::NodeKey>> =
        use_signal(Vec::new);
    use_context_provider(|| crate::local_mode::explorer::VisibleFlat(visible_flat));
    let drag_session: Signal<Option<DragKind>> = use_signal(|| None);
    use_context_provider(|| DragSession(drag_session));
    // Plans-Phase-3-explorer-drag-drop-feedback: descendant set of the
    // currently dragged note. Populated on dragstart, cleared on
    // dragend/drop. Note rows read this to reject drops that would create
    // a cycle (drop a note onto its own subtree).
    let drag_descendants: Signal<std::collections::BTreeSet<uuid::Uuid>> =
        use_signal(std::collections::BTreeSet::new);
    use_context_provider(|| crate::local_mode::ui::DragDescendants(drag_descendants));
    let clipboard: Signal<Option<Clipboard>> = use_signal(|| None);
    use_context_provider(|| LocalClipboard(clipboard));
    let bulk_clipboard: Signal<Option<crate::local_mode::ui::BulkClipboard>> = use_signal(|| None);
    use_context_provider(|| crate::local_mode::ui::LocalBulkClipboard(bulk_clipboard));
    // Workspace tree-state — shared between the explorer panel and the
    // dedicated search panel so a click in either expands the matching
    // project consistently.
    let LocalTreeStateRepo(tree_repo) = use_context();
    let workspace_open: Signal<HashMap<String, bool>> = use_signal(|| {
        tree_repo
            .snapshot_for_scope("workspace")
            .unwrap_or_default()
    });
    use_context_provider(|| WorkspaceOpenMap(workspace_open));
    let tree_queue: Signal<TreeStateQueue> =
        use_signal(|| TreeStateQueue::new(tree_repo.clone()));
    use_context_provider(|| WorkspaceTreeQueueCtx(tree_queue));

    // Cross-component reveal request. Companion-chat mention chips and
    // the markdown note-link resolver write the target note's id here;
    // an effect inside `ExplorerPanel` reads it, expands ancestors, and
    // clears it back to None.
    let reveal_note_request: Signal<Option<Uuid>> = use_signal(|| None);
    use_context_provider(|| {
        crate::local_mode::explorer::RevealNoteRequest(reveal_note_request)
    });
    // Re-hydrate `workspace_open` whenever the project list changes — covers
    // freshly-created projects whose state wasn't fetched on first mount.
    {
        let mut workspace_open_setter = workspace_open;
        let repo = tree_repo.clone();
        use_effect(move || {
            let _ = project_version.read();
            match repo.snapshot_for_scope("workspace") {
                Ok(snap) => workspace_open_setter.set(snap),
                Err(e) => eprintln!("operon: tree-state snapshot failed: {e}"),
            }
        });
    }

    // M1-companion-claude-code: keep `active_repo_path` in sync with the
    // currently-selected project. Re-runs on project selection changes AND on
    // any project mutation (project_version) so a "Set Repository…" click
    // immediately reaches the companion plugin.
    {
        let LocalProjectRepo(project_repo_for_active) =
            use_context::<LocalProjectRepo>();
        let mut active_setter = active_repo_path;
        let selected = selected_project;
        use_effect(move || {
            let pid = *selected.read();
            let _ = project_version.read();
            let next = pid.and_then(|id| {
                project_repo_for_active
                    .list()
                    .ok()
                    .and_then(|projects| projects.into_iter().find(|p| p.id == id))
                    .and_then(|p| p.repo_path)
            });
            active_setter.set(next);
        });
    }

    // PostToolUse reload hook: bind the per-process Unix socket once
    // at app start, then install / refresh the hook entry in
    // `<repo>/.claude/settings.local.json` whenever the bound repo
    // changes. Skipped silently when the hook binary can't be
    // resolved (e.g. running from a build that didn't ship the
    // sidecar) — the inotify watchers remain as the fallback.
    {
        let socket_for_install =
            crate::local_mode::reload_socket::start();
        let active_for_hook = active_repo_path;
        use_effect(move || {
            let Some(repo) = active_for_hook.read().clone() else {
                return;
            };
            let Some(socket) = socket_for_install.clone() else {
                return;
            };
            let Some(hook_bin) =
                crate::shell::companion_chat::resolve_operon_posttool_hook()
            else {
                tracing::warn!(
                    target: "operon::posttool_hook",
                    "operon-posttool-hook binary not found; PostToolUse reloads disabled"
                );
                return;
            };
            if let Err(e) = crate::shell::posttool_hook::install(
                &repo, &hook_bin, &socket,
            ) {
                tracing::warn!(
                    target: "operon::posttool_hook",
                    "install hook for {}: {e}",
                    repo.display()
                );
            }
        });
    }

    let local_search_focus_tick: Signal<u64> = use_signal(|| 0);
    use_context_provider(|| {
        crate::plugins::local_search::LocalSearchFocus(local_search_focus_tick)
    });

    let scheduler: crate::tabs::SaveScheduler = use_context();
    let LocalNoteLinkRepo(link_repo) = use_context::<LocalNoteLinkRepo>();
    let LocalProjectRepo(project_repo_for_save) = use_context::<LocalProjectRepo>();
    let save_callback = use_hook(|| {
        install_save_action(
            tabs,
            persistence.clone(),
            note_repo.clone(),
            project_repo_for_save.clone(),
            link_repo.clone(),
            scheduler.clone(),
        )
    });
    use_context_provider(|| LocalSaveAction {
        callback: save_callback,
    });

    // Plans-Phase-5-vfs-wikilinks: install a click resolver so wikilink
    // anchors rendered inside MarkdownView open the linked note in a tab.
    let LocalProjectRepo(project_repo) = use_context::<LocalProjectRepo>();
    let LocalNoteRepo(note_repo_for_links) = use_context::<LocalNoteRepo>();
    let SelectedNote(selected_note_for_links) = use_context::<SelectedNote>();
    let project_repo_for_links = project_repo.clone();
    let note_repo_for_links_resolver = note_repo_for_links.clone();
    let tabs_for_links = tabs;
    let scheduler_for_links = scheduler.clone();
    let mut selected_note_for_links_setter = selected_note_for_links;
    let wikilink_resolver = use_hook(move || {
        Callback::new(move |target: String| {
            // Heuristic source project: the currently selected note's
            // project, otherwise the first project. Fine for first cut.
            let snap_projects = project_repo_for_links.list().unwrap_or_default();
            let source_project_id = (*selected_note_for_links_setter.read())
                .and_then(|nid| {
                    snap_projects.iter().find_map(|p| {
                        note_repo_for_links_resolver
                            .list_for_project(p.id)
                            .ok()
                            .and_then(|notes| notes.iter().find(|n| n.id == nid).map(|_| p.id))
                    })
                })
                .or_else(|| snap_projects.first().map(|p| p.id));
            let Some(source_project_id) = source_project_id else {
                eprintln!("operon: wikilink click — no project context");
                return;
            };
            let Some(form) = vfs::parse_link(&target) else {
                return;
            };
            match vfs::resolve_link(
                project_repo_for_links.as_ref(),
                note_repo_for_links_resolver.as_ref(),
                source_project_id,
                &form,
            ) {
                Ok(note_id) => {
                    let (title, kind) = note_repo_for_links_resolver
                        .list_for_project(source_project_id)
                        .ok()
                        .and_then(|notes| {
                            notes
                                .into_iter()
                                .find(|n| n.id == note_id)
                                .map(|n| (n.title, n.kind))
                        })
                        .unwrap_or_else(|| (target.clone(), NoteKind::Markdown));
                    super::editor::open_local_note_tab(
                        tabs_for_links,
                        scheduler_for_links.clone(),
                        note_id,
                        title,
                        String::new(),
                        kind,
                    );
                    selected_note_for_links_setter.set(Some(note_id));
                }
                Err(e) => eprintln!("operon: wikilink resolve failed for {target:?}: {e}"),
            }
        })
    });
    use_context_provider(|| crate::plugins::markdown::render::WikiLinkResolver(wikilink_resolver));

    // Click resolver for `[text](operon://note/<uuid>)` markdown links
    // emitted by the artifact-view link picker. Resolution is direct
    // (no title parsing) — uuid → first project that contains it →
    // open in a tab. The `WikiLinkResolver` above handles the legacy
    // `[[Project/Title^short]]` form; this is its uuid-keyed sibling.
    let note_repo_for_note_link = note_repo_for_links.clone();
    let project_repo_for_note_link = project_repo.clone();
    let tabs_for_note_link = tabs;
    let scheduler_for_note_link = scheduler.clone();
    let mut selected_note_for_note_link_setter = selected_note_for_links;
    let mut reveal_request_for_note_link = reveal_note_request;
    let note_link_resolver = use_hook(move || {
        Callback::new(move |note_id: Uuid| {
            let snap_projects = project_repo_for_note_link.list().unwrap_or_default();
            for p in &snap_projects {
                if let Ok(notes) = note_repo_for_note_link.list_for_project(p.id) {
                    if let Some(note) = notes.into_iter().find(|n| n.id == note_id) {
                        super::editor::open_local_note_tab(
                            tabs_for_note_link,
                            scheduler_for_note_link.clone(),
                            note_id,
                            note.title,
                            String::new(),
                            note.kind,
                        );
                        selected_note_for_note_link_setter.set(Some(note_id));
                        // Ask the explorer to expand the owning project
                        // + walk the ancestor chain so the note is
                        // actually visible (the tab open + selection
                        // alone don't expand collapsed parents).
                        reveal_request_for_note_link.set(Some(note_id));
                        return;
                    }
                }
            }
            eprintln!("operon: operon-note click \u{2014} no project contains note {note_id}");
        })
    });
    use_context_provider(|| {
        crate::plugins::markdown::render::NoteLinkResolver(note_link_resolver)
    });

    // Sibling resolver for *current* note title. Companion chat mention
    // chips read this through `try_consume_context` so a rename in the
    // explorer re-renders every chat chip referencing the note. Subscribes
    // to `NOTE_TITLE_VERSION` (bumped by the rename path) inside the
    // callback body — calling the callback inside an rsx scope auto-
    // subscribes that scope to the signal.
    let note_repo_for_title = note_repo_for_links.clone();
    let project_repo_for_title = project_repo.clone();
    let note_title_resolver = use_hook(move || {
        Callback::new(move |note_id: Uuid| -> Option<String> {
            let _ = crate::shell::companion_state::NOTE_TITLE_VERSION.read();
            let project_repo_opt: Option<&Arc<dyn LocalProjectRepository>> =
                Some(&project_repo_for_title);
            crate::local_mode::note_lookup::lookup_note_title(
                &note_repo_for_title,
                project_repo_opt,
                note_id,
            )
        })
    });
    use_context_provider(|| {
        crate::plugins::markdown::render::NoteTitleResolver(note_title_resolver)
    });

    // Plans-Phase-9-wikilink-picker (rev 3): shared per-shell cache for
    // the WikiLinkChecker + WikiLinkImageResolver. Both run on every
    // MarkdownView render; in Split mode that's once per keystroke. The
    // image resolver's read_image + base64_encode dominates wall-time
    // (each call did multi-MB disk I/O + string alloc), and a body with
    // even one embed could lock the UI on every keystroke — exactly the
    // "paste freezes the app" symptom. Cache by target string;
    // invalidate by tracking `note_version` + `project_version`. Both
    // bump on rename / move / create / delete, so a stale entry never
    // outlives the underlying file.
    #[derive(Default)]
    struct WikilinkCache {
        observed_note_version: u64,
        observed_project_version: u64,
        check: HashMap<String, bool>,
        image: HashMap<String, Option<String>>,
    }

    let cache: Rc<RefCell<WikilinkCache>> = use_hook(|| Rc::new(RefCell::new(WikilinkCache::default())));
    let LocalNoteVersion(note_version_for_cache) = use_context::<LocalNoteVersion>();
    let LocalProjectVersion(project_version_for_cache) = use_context::<LocalProjectVersion>();

    // Plans-Phase-5-vfs-wikilinks: sync checker the renderer calls during
    // render to flag broken `[[…]]` links. Returns true on a unique resolve.
    let project_repo_for_check = project_repo.clone();
    let note_repo_for_check = note_repo_for_links.clone();
    let selected_note_for_check = selected_note_for_links;
    let cache_for_check = cache.clone();
    let wikilink_checker = use_hook(move || {
        Callback::new(move |target: String| -> bool {
            // peek() reads the current version without subscribing the
            // callback to it — the version change invalidates the cache
            // lazily on the next call instead of triggering a re-render
            // loop here.
            let nv = *note_version_for_cache.peek();
            let pv = *project_version_for_cache.peek();
            {
                let mut state = cache_for_check.borrow_mut();
                if state.observed_note_version != nv || state.observed_project_version != pv {
                    state.observed_note_version = nv;
                    state.observed_project_version = pv;
                    state.check.clear();
                    state.image.clear();
                }
                if let Some(hit) = state.check.get(&target).copied() {
                    return hit;
                }
            }
            let snap_projects = project_repo_for_check.list().unwrap_or_default();
            let source_project_id = (*selected_note_for_check.read())
                .and_then(|nid| {
                    snap_projects.iter().find_map(|p| {
                        note_repo_for_check
                            .list_for_project(p.id)
                            .ok()
                            .and_then(|notes| notes.iter().find(|n| n.id == nid).map(|_| p.id))
                    })
                })
                .or_else(|| snap_projects.first().map(|p| p.id));
            let result = match source_project_id {
                None => false,
                Some(spid) => match vfs::parse_link(&target) {
                    None => false,
                    Some(form) => matches!(
                        vfs::resolve_link(
                            project_repo_for_check.as_ref(),
                            note_repo_for_check.as_ref(),
                            spid,
                            &form,
                        ),
                        Ok(_)
                    ),
                },
            };
            cache_for_check.borrow_mut().check.insert(target, result);
            result
        })
    });
    use_context_provider(|| crate::plugins::markdown::render::WikiLinkChecker(wikilink_checker));

    // Plans-Phase-6-image-notes (inline-embed): resolver that turns an
    // `![[Title^short]]` embed target into a `data:` URL when it points
    // to an image-note blob. The MarkdownView renderer consumes this
    // context to emit `<img>` instead of the text-anchor fallback. Reads
    // are cached above on first hit so subsequent renders skip
    // `read_image` + base64 entirely.
    let project_repo_for_img = project_repo.clone();
    let note_repo_for_img = note_repo_for_links.clone();
    let selected_note_for_img = selected_note_for_links;
    let CurrentVaultRoot(vault_for_img) = use_context::<CurrentVaultRoot>();
    let cache_for_image = cache;
    let wikilink_image_resolver = use_hook(move || {
        Callback::new(move |target: String| -> Option<String> {
            let nv = *note_version_for_cache.peek();
            let pv = *project_version_for_cache.peek();
            {
                let mut state = cache_for_image.borrow_mut();
                if state.observed_note_version != nv || state.observed_project_version != pv {
                    state.observed_note_version = nv;
                    state.observed_project_version = pv;
                    state.check.clear();
                    state.image.clear();
                }
                if let Some(hit) = state.image.get(&target).cloned() {
                    return hit;
                }
            }
            let computed = (|| -> Option<String> {
                let snap_projects = project_repo_for_img.list().ok()?;
                let source_project_id = (*selected_note_for_img.read())
                    .and_then(|nid| {
                        snap_projects.iter().find_map(|p| {
                            note_repo_for_img
                                .list_for_project(p.id)
                                .ok()
                                .and_then(|notes| notes.iter().find(|n| n.id == nid).map(|_| p.id))
                        })
                    })
                    .or_else(|| snap_projects.first().map(|p| p.id))?;
                let form = vfs::parse_link(&target)?;
                let note_id = vfs::resolve_link(
                    project_repo_for_img.as_ref(),
                    note_repo_for_img.as_ref(),
                    source_project_id,
                    &form,
                )
                .ok()?;
                let row = note_repo_for_img
                    .list_for_project(source_project_id)
                    .ok()?
                    .into_iter()
                    .find(|n| n.id == note_id)?;
                if !matches!(row.kind, NoteKind::Image) {
                    return None;
                }
                let blob_path = row.blob_path.clone()?;
                let vault = vault_for_img.read().clone()?;
                crate::local_mode::images::data_url_for_blob(
                    &vault,
                    std::path::Path::new(&blob_path),
                )
            })();
            cache_for_image.borrow_mut().image.insert(target, computed.clone());
            computed
        })
    });
    use_context_provider(|| {
        crate::plugins::markdown::render::WikiLinkImageResolver(wikilink_image_resolver)
    });

    // Standard `![alt](path)` resolver: when `path` is a vault-relative
    // blob (e.g. `.operon/images/<sha>.png` produced by paste-image), turn
    // it into a `data:` URL so wry's webview can actually render it.
    // Also recognises `operon://note/<uuid>` links — when the target
    // resolves to a `NoteKind::Image` row, we look up its blob and
    // return the data URL the same way `WikiLinkImageResolver` does
    // for `![[Title^short]]` embeds. Everything else (external URLs,
    // missing notes, unresolvable paths) passes through as `None`,
    // leaving the literal `dest` on the `<img>`.
    let vault_for_md_img = vault_for_img;
    let note_repo_for_md_img = note_repo_for_links.clone();
    let project_repo_for_md_img = project_repo.clone();
    let markdown_image_resolver = use_hook(move || {
        Callback::new(move |dest: String| -> Option<String> {
            let trimmed = dest.trim();
            if trimmed.is_empty() {
                return None;
            }
            // operon:// scheme — resolve to an image-kind note's blob.
            if let Some(note_id) =
                crate::plugins::markdown::render::parse_operon_note_url(trimmed)
            {
                let snap_projects =
                    project_repo_for_md_img.list().unwrap_or_default();
                for p in &snap_projects {
                    let notes =
                        match note_repo_for_md_img.list_for_project(p.id) {
                            Ok(n) => n,
                            Err(_) => continue,
                        };
                    let row = match notes.into_iter().find(|n| n.id == note_id) {
                        Some(r) => r,
                        None => continue,
                    };
                    if !matches!(row.kind, NoteKind::Image) {
                        return None;
                    }
                    let blob_path = row.blob_path.clone()?;
                    let vault = vault_for_md_img.read().clone()?;
                    return crate::local_mode::images::data_url_for_blob(
                        &vault,
                        std::path::Path::new(&blob_path),
                    );
                }
                return None;
            }
            // Bypass absolute URLs and already-resolved data URLs.
            let lower = trimmed.to_ascii_lowercase();
            if lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("data:")
                || lower.starts_with("file://")
                || lower.starts_with("bridge://")
            {
                return None;
            }
            let vault = vault_for_md_img.read().clone()?;
            // Strip a leading `./` and treat the rest as vault-relative.
            let rel = trimmed.strip_prefix("./").unwrap_or(trimmed);
            crate::local_mode::images::data_url_for_blob(
                &vault,
                std::path::Path::new(rel),
            )
        })
    });
    use_context_provider(|| {
        crate::plugins::markdown::render::MarkdownImageResolver(markdown_image_resolver)
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
    let crate::local_mode::ui::LocalBulkClipboard(bulk_clipboard) = use_context();
    let crate::local_mode::explorer::MultiSelected(multi_selected) = use_context();
    let SelectedProject(selected_project) = use_context();
    let SelectedNote(selected_note) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let crate::plugins::local_search::LocalSearchFocus(local_search_focus_tick) = use_context();
    let crate::shell::state::ActiveActivity(active_activity) = use_context();
    let SettingsOpen(settings_open) = use_context();
    let LocalUsername(username) = use_context();
    // Plans-Phase-9-monaco-desktop (rev 14): `tabs` and `save_action`
    // are still consumed via context elsewhere (`LocalNoteEditor`,
    // `MainArea`); we keep the `use_context()` calls so the providers
    // upstream remain wired, even though `LocalShellOverlay` no
    // longer references either after the floating Save button was
    // removed.
    let _tabs: Signal<TabManager> = use_context();
    let _save_action: LocalSaveAction = use_context();

    let mut clipboard_setter = clipboard;
    let mut bulk_clipboard_setter = bulk_clipboard;
    let mut multi_selected_setter = multi_selected;
    let mut selected_project_setter = selected_project;
    let mut note_version_setter = note_version;
    let mut local_search_focus_tick_setter = local_search_focus_tick;
    let mut active_activity_setter = active_activity;
    let _ = username;
    let _ = settings_open;
    let note_repo_for_keys = note_repo.clone();
    let project_repo_for_keys = project_repo.clone();

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
                    // Activate the dedicated Search panel and bump its focus
                    // tick so its input grabs focus whether the panel is
                    // freshly mounted or already visible.
                    active_activity_setter.set(Some(crate::shell::state::ActivityItemId(
                        "local-search:default".to_string(),
                    )));
                    local_search_focus_tick_setter.with_mut(|t| *t += 1);
                    return;
                }
                if with_meta && !mods.contains(Modifiers::ALT) && !mods.contains(Modifiers::SHIFT) {
                    if key.eq_ignore_ascii_case("x") || key.eq_ignore_ascii_case("c") {
                        let kind = if key.eq_ignore_ascii_case("x") {
                            ClipKind::Cut
                        } else {
                            ClipKind::Copy
                        };
                        // Plans-Phase-4-multiselect-aria: multi-set takes
                        // precedence when 2+ items are selected.
                        let multi: Vec<ClipPayload> = multi_selected
                            .read()
                            .iter()
                            .map(|k| match k {
                                crate::local_mode::explorer::NodeKey::Note(id) => {
                                    ClipPayload::Note(*id)
                                }
                                crate::local_mode::explorer::NodeKey::Project(id) => {
                                    ClipPayload::Project(*id)
                                }
                            })
                            .collect();
                        if multi.len() >= 2 {
                            bulk_clipboard_setter.set(Some(crate::local_mode::ui::BulkClipboard {
                                kind,
                                items: multi,
                            }));
                            clipboard_setter.set(None);
                            evt.prevent_default();
                            return;
                        }
                        let payload = if let Some(nid) = *selected_note.read() {
                            Some(ClipPayload::Note(nid))
                        } else {
                            (*selected_project.read()).map(ClipPayload::Project)
                        };
                        if let Some(payload) = payload {
                            clipboard_setter.set(Some(Clipboard { kind, payload }));
                            bulk_clipboard_setter.set(None);
                            evt.prevent_default();
                            return;
                        }
                    }
                    if key.eq_ignore_ascii_case("v") {
                        let bulk = bulk_clipboard.read().clone();
                        if let Some(bulk) = bulk {
                            for payload in bulk.items.iter() {
                                paste_clipboard(
                                    Clipboard {
                                        kind: bulk.kind,
                                        payload: *payload,
                                    },
                                    *selected_note.read(),
                                    *selected_project.read(),
                                    &note_repo_for_keys,
                                    &project_repo_for_keys,
                                );
                            }
                            note_version_setter.with_mut(|v| *v += 1);
                            if matches!(bulk.kind, ClipKind::Cut) {
                                bulk_clipboard_setter.set(None);
                                multi_selected_setter
                                    .set(std::collections::BTreeSet::new());
                            }
                            evt.prevent_default();
                            return;
                        }
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
                if key == "Escape" {
                    if clipboard.read().is_some() {
                        clipboard_setter.set(None);
                        evt.prevent_default();
                        return;
                    }
                    if bulk_clipboard.read().is_some() {
                        bulk_clipboard_setter.set(None);
                        evt.prevent_default();
                        return;
                    }
                }
                let _ = &mut selected_project_setter;
            },
            {children}
            // Plans-Phase-5-vfs-wikilinks: backlinks pane. Renders a list
            // of notes referencing the active selection. Renders nothing
            // when the selection has no referrers.
            div {
                style: "position: fixed; right: 12px; bottom: 36px; max-width: 24rem; \
                        background: var(--operon-bg, #fff); border: 1px solid var(--operon-border); \
                        border-radius: 0.25rem; box-shadow: 0 1px 4px rgba(0,0,0,0.08); z-index: 30;",
                crate::local_mode::explorer::BacklinksPanel {}
            }
            // Plans-Phase-9-monaco-desktop (rev 14): the floating
            // Save button is gone — Cmd/Ctrl+S routes through Monaco's
            // window-capture keydown listener and dispatches a
            // `save` keyaction back to `LocalNoteEditor::on_action`,
            // which calls the same `LocalSaveAction.callback`. The
            // tab's own dirty bullet remains as the visual save-state
            // indicator.
            if *settings_open.read() {
                SettingsPanel {
                    open: settings_open,
                    username,
                }
            }
        }
    }
}

/// Slice A14 cutover + Slice A4b: assemble the `BackendsCtx` and the
/// `SettingsService` together, sharing one `LayeredSecretStore`. The
/// factory closure used to construct per-session runtimes captures the
/// same store, so a key written from the settings page is visible the
/// next time `bind_session` causes a runtime to be built.
fn build_backends_and_settings(
    claude_plugin: std::sync::Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>,
) -> (
    crate::shell::companion_state::BackendsCtx,
    crate::shell::settings::SettingsService,
) {
    use operon_core::agent_event::AgentBackend;
    use std::sync::Arc;
    eprintln!("[desktop] build_backends_and_settings invoked");

    // Claude-code adapter: the existing Arc already implements `AgentBackend`
    // via the `agent_backend.rs` adapter — coerce to the trait object.
    let claude_code: Arc<dyn AgentBackend> = claude_plugin;

    // Layered secret store: keyring (OS) → JSON file → env vars → in-memory.
    // The JSON-file layer (mode 0600 at `$XDG_CONFIG_HOME/operon/secrets.json`)
    // is the persistent fallback for machines where Secret Service / KWallet
    // is locked or unavailable. Env vars stay read-only. The in-memory mock
    // is last-resort so `put` never fails outright.
    let secrets: Arc<dyn operon_core::secrets::SecretStore> = Arc::new(
        operon_core::secrets::LayeredSecretStore::new(vec![
            Arc::new(operon_core::secrets::KeyringSecretStore::new("operon")),
            Arc::new(operon_core::secrets::JsonFileSecretStore::new(
                operon_core::secrets::JsonFileSecretStore::default_path(),
            )),
            Arc::new(operon_core::secrets::EnvSecretStore::new("")),
            Arc::new(operon_core::secrets::MockSecretStore::new()),
        ]),
    );

    let factory: operon_plugins_tools::RuntimeFactory = {
        let secrets = secrets.clone();
        Arc::new(move |_args| {
            let chat = operon_plugins_anthropic::AnthropicChatPlugin::new(
                operon_plugins_anthropic::AnthropicConfig::default(),
                secrets.clone(),
            )?;
            let chat: Arc<dyn operon_core::traits::ChatPlugin> = Arc::new(chat);
            let tools = operon_plugins_tools::default_tools();
            let memory: Arc<dyn operon_core::traits::MemoryPlugin> =
                Arc::new(operon_core::InMemoryStore::new());
            let bus = operon_core::EventBus::new(64);
            Ok(Arc::new(operon_core::runtime::AgentRuntime::new(
                chat, tools, memory, bus,
            )))
        })
    };
    let runtime: Arc<dyn AgentBackend> =
        Arc::new(operon_plugins_tools::RuntimeAgentBackend::new(factory));

    let settings_service = crate::shell::settings::SettingsService::new(secrets);

    (
        crate::shell::companion_state::BackendsCtx {
            claude_code,
            runtime,
        },
        settings_service,
    )
}
