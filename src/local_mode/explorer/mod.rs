//! Local-Mode explorer panel: lists `local_project` rows with rename/delete and
//! a "+" button to create a new (default-named) project. Phase 3 nests notes
//! under each project, persisted via `local_note` + `local_tree_state`.

mod bulk_rename;
mod note_row;
mod project_row;
mod search;
mod tree_node;
mod tree_state;

pub use bulk_rename::BulkRenameModal;

pub use note_row::NoteRow;
pub use project_row::ProjectRow;
pub use search::{
    click_handler as search_click_handler, load_body_cache, BodyCache, ExplorerSearch,
    ExplorerSearchFocus, ExplorerSearchRepo, ResultsList,
};
pub use tree_node::{flatten_visible, NoteForest};
pub use tree_state::TreeStateQueue;

use std::collections::HashMap;
use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::{LocalNote, LocalProject, NoteKind};
use uuid::Uuid;

use crate::editor::EditorMode;
use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo, LocalTreeStateRepo};
use crate::local_mode::editor::{open_local_note_tab, LocalSaveAction};
use crate::local_mode::ui::{
    ClipKind, ClipPayload, Clipboard, ConfirmDialog, DragKind, DragSession, DropPosition,
    LocalClipboard,
};
use crate::persistence::Persistence;
use crate::tabs::{SaveScheduler, TabManager};

/// App-scope signal: bumped on every successful project mutation. The panel
/// re-fetches its row list whenever this changes.
#[derive(Clone, Copy)]
pub struct LocalProjectVersion(pub Signal<u64>);

/// App-scope signal: id of the currently selected project, if any.
#[derive(Clone, Copy)]
pub struct SelectedProject(pub Signal<Option<Uuid>>);

/// App-scope signal: id of the currently selected (and open in tab) note, if any.
#[derive(Clone, Copy)]
pub struct SelectedNote(pub Signal<Option<Uuid>>);

/// Plans-Phase-4-multiselect-aria: which row(s) participate in bulk
/// operations. `Note` and `Project` flavors both live in the same set so
/// that group DnD and bulk-delete can mix-and-match (subject to the same
/// project / cycle constraints handled by the underlying repos).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum NodeKey {
    Project(Uuid),
    Note(Uuid),
}

/// App-scope signal: the active multi-selection set. Empty when only
/// single-select (the existing `SelectedNote` / `SelectedProject` signals)
/// is in play; populated when the user Ctrl/Cmd+clicks or Shift+clicks rows.
/// When non-empty, the explorer renders a bulk-action toolbar.
#[derive(Clone, Copy)]
pub struct MultiSelected(pub Signal<std::collections::BTreeSet<NodeKey>>);

/// Track the most recently clicked row so Shift+click can compute a range
/// over the visible flattened tree.
#[derive(Clone, Copy)]
pub struct LastClicked(pub Signal<Option<NodeKey>>);

/// Plans-Phase-4-multiselect-aria: visible flattened tree across all
/// projects, in document order, respecting open/closed state. Updated by
/// ExplorerPanel whenever its inputs change; NoteRow / ProjectRow consume
/// it during Shift+click to compute proper ranges.
#[derive(Clone, Copy)]
pub struct VisibleFlat(pub Signal<Vec<NodeKey>>);

/// Bumped on every note mutation. The panel re-fetches notes for the
/// affected project when this changes.
#[derive(Clone, Copy)]
pub struct LocalNoteVersion(pub Signal<u64>);

const SCOPE_WORKSPACE: &str = "workspace";

/// Plans-Phase-4-multiselect-aria: replace OS-unsafe characters with `_` so
/// titles can be used as filenames during bulk export.
fn sanitize_filename(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    if out.trim().is_empty() {
        out = "Untitled".to_string();
    }
    out
}

/// Plans-Phase-6-image-notes: shared helper used by both the note-row and
/// project-row image-drop callbacks. Writes the bytes via
/// [`crate::local_mode::images::write_image`], mints an image-note via
/// `create_with_kind`, stamps `blob_path`, and bumps `note_version`.
fn handle_image_drop(
    project_id: Uuid,
    parent_id: Option<Uuid>,
    bytes: Vec<u8>,
    filename: String,
    vault: &crate::local_mode::vault::VaultRoot,
    note_repo: &Arc<dyn operon_store::repos::LocalNoteRepository>,
    mut note_version: Signal<u64>,
) {
    let lower = filename.to_ascii_lowercase();
    let mime = if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".avif") {
        "image/avif"
    } else {
        eprintln!("operon: image drop: unsupported extension in {filename}");
        return;
    };
    let written = match crate::local_mode::images::write_image(vault, &bytes, mime) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("operon: image drop write_image failed: {e}");
            return;
        }
    };
    let stem = std::path::Path::new(&filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Image")
        .to_string();
    match note_repo.create_with_kind(project_id, parent_id, &stem, NoteKind::Image) {
        Ok(row) => {
            let rel = written.relative_path.to_string_lossy().to_string();
            if let Err(e) = note_repo.set_blob_path(row.id, Some(&rel)) {
                eprintln!("operon: image drop set_blob_path failed: {e}");
            }
            note_version.with_mut(|v| *v += 1);
        }
        Err(e) => eprintln!("operon: image drop create_with_kind failed: {e}"),
    }
}

/// Pick a unique path inside `dir` with `stem.ext`; appends ` (2)` etc on
/// collision so a bulk export of repeat-titled notes doesn't overwrite.
fn unique_path(dir: &std::path::Path, stem: &str, ext: &str) -> std::path::PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}.{ext}"))
}

fn scope_for_project(project_id: Uuid) -> String {
    format!("project:{project_id}")
}

#[component]
pub fn ExplorerPanel() -> Element {
    let LocalProjectRepo(project_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let LocalTreeStateRepo(tree_repo) = use_context();
    let LocalProjectVersion(mut project_version) = use_context();
    let LocalNoteVersion(mut note_version) = use_context();
    let SelectedProject(selected_project) = use_context();
    let mut selected_project = selected_project;
    let SelectedNote(mut selected_note) = use_context();
    let DragSession(drag_session) = use_context();
    let LocalClipboard(clipboard) = use_context();
    let tabs: Signal<TabManager> = use_context();
    let save_scheduler: SaveScheduler = use_context();
    let _save_action: LocalSaveAction = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let ExplorerSearchFocus(focus_tick) = use_context();

    // Re-fetch project list when version bumps.
    let projects: Signal<Vec<LocalProject>> = use_signal(Vec::new);
    let mut projects_setter = projects;
    {
        let repo = project_repo.clone();
        use_effect(move || {
            let _ = project_version.read();
            match repo.list() {
                Ok(rows) => projects_setter.set(rows),
                Err(e) => eprintln!("operon: list local_project failed: {e}"),
            }
        });
    }

    // Workspace-scope tree-state snapshot (which projects are open).
    let workspace_open: Signal<HashMap<String, bool>> = use_signal(HashMap::new);
    let mut workspace_open_setter = workspace_open;
    {
        let repo = tree_repo.clone();
        use_effect(move || {
            // Re-hydrate when the project list changes (covers freshly-created
            // projects whose state wasn't fetched on first mount).
            let _ = project_version.read();
            match repo.snapshot_for_scope(SCOPE_WORKSPACE) {
                Ok(snap) => workspace_open_setter.set(snap),
                Err(e) => eprintln!("operon: tree-state snapshot failed: {e}"),
            }
        });
    }

    // Per-project note lists, keyed by project_id. Re-fetched on note_version bump.
    let notes_by_project: Signal<HashMap<Uuid, Vec<LocalNote>>> = use_signal(HashMap::new);
    let mut notes_setter = notes_by_project;
    {
        let repo = note_repo.clone();
        use_effect(move || {
            let _ = note_version.read();
            let project_list = projects.read().clone();
            let mut map = HashMap::new();
            for p in project_list.iter() {
                match repo.list_for_project(p.id) {
                    Ok(rows) => {
                        map.insert(p.id, rows);
                    }
                    Err(e) => eprintln!("operon: list_for_project {} failed: {e}", p.id),
                }
            }
            notes_setter.set(map);
        });
    }

    // Per-project note open/closed snapshots, lazily hydrated when a project opens.
    let project_note_open: Signal<HashMap<Uuid, HashMap<String, bool>>> = use_signal(HashMap::new);

    // Tree-state debounce queue.
    let tree_queue: Signal<TreeStateQueue> = use_signal(|| TreeStateQueue::new(tree_repo.clone()));

    // ===== Phase-5: search state =====
    let search_query: Signal<String> = use_signal(String::new);
    let search_in_content: Signal<bool> = use_signal(|| false);
    // Debounced query — only flushed 150ms after the user stops typing.
    let debounced_query: Signal<String> = use_signal(String::new);
    let body_cache: Signal<BodyCache> = use_signal(BodyCache::default);
    // Snapshot of the previously-selected note so Esc can restore focus.
    let prev_selection: Signal<Option<Uuid>> = use_signal(|| None);

    // Debounce: spawn a delay each time the query changes; only the last spawn
    // wins by checking a generation counter.
    {
        let mut debounced_setter = debounced_query;
        use_effect(move || {
            let q = search_query.read().clone();
            spawn(async move {
                search::debounce_window().await;
                debounced_setter.set(q);
            });
        });
    }

    // Body cache lifecycle: load when in_content flips on; clear when off.
    {
        let mut body_cache_setter = body_cache;
        let p_repo = project_repo.clone();
        let n_repo = note_repo.clone();
        let persistence_for_cache = persistence.clone();
        use_effect(move || {
            let on = *search_in_content.read();
            if !on {
                body_cache_setter.set(BodyCache::default());
                return;
            }
            let Ok(projects) = p_repo.list() else {
                return;
            };
            let mut all_ids: Vec<Uuid> = Vec::new();
            for p in projects {
                if let Ok(rows) = n_repo.list_for_project(p.id) {
                    all_ids.extend(rows.iter().map(|r| r.id));
                }
            }
            let persistence = persistence_for_cache.clone();
            spawn(async move {
                let map = search::load_body_cache(all_ids, persistence).await;
                body_cache_setter.set(BodyCache(Arc::new(map)));
            });
        });
    }

    // Track focus_tick → focus the input. The ExplorerSearch component reads
    // the focus_tick signal in its onmounted.
    let _ = focus_tick;

    let renaming_project: Signal<Option<Uuid>> = use_signal(|| None);
    let pending_delete_project: Signal<Option<Uuid>> = use_signal(|| None);
    let renaming_note: Signal<Option<Uuid>> = use_signal(|| None);
    let pending_delete_note: Signal<Option<Uuid>> = use_signal(|| None);
    // Plans-Phase-4-multiselect-aria: when set, render a confirm modal
    // listing the count of selected items to be bulk-deleted.
    let pending_bulk_delete: Signal<bool> = use_signal(|| false);
    // Plans-Phase-4-multiselect-aria: bulk rename modal visibility.
    let mut pending_bulk_rename: Signal<bool> = use_signal(|| false);
    let mut renaming_project_setter = renaming_project;
    let mut pending_delete_project_setter = pending_delete_project;
    let mut renaming_note_setter = renaming_note;
    let mut pending_delete_note_setter = pending_delete_note;
    let mut pending_bulk_delete_setter = pending_bulk_delete;
    let MultiSelected(mut multi_selected) = use_context();
    let multi_selected_for_render = multi_selected;

    // ===== Project handlers =====
    let on_select_project = use_callback(move |id: Uuid| {
        selected_project.set(Some(id));
    });

    let project_repo_for_create = project_repo.clone();
    let on_add_project = move |_| match project_repo_for_create.create("") {
        Ok(p) => {
            project_version.with_mut(|v| *v += 1);
            selected_project.set(Some(p.id));
            renaming_project_setter.set(Some(p.id));
        }
        Err(e) => eprintln!("operon: create local_project failed: {e}"),
    };

    let project_repo_for_rename = project_repo.clone();
    let on_rename_project = use_callback(move |(id, new_name): (Uuid, String)| {
        if new_name.trim().is_empty() {
            renaming_project_setter.set(None);
            return;
        }
        match project_repo_for_rename.rename(id, &new_name) {
            Ok(()) => {
                project_version.with_mut(|v| *v += 1);
                renaming_project_setter.set(None);
            }
            Err(e) => {
                eprintln!("operon: rename local_project failed: {e}");
                renaming_project_setter.set(None);
            }
        }
    });

    let on_request_rename_project = use_callback(move |id: Uuid| {
        renaming_project_setter.set(Some(id));
    });
    let on_request_delete_project = use_callback(move |id: Uuid| {
        pending_delete_project_setter.set(Some(id));
    });
    let on_delete_project_noop = use_callback(move |_id: Uuid| {});

    // Toggle project open/closed; persists via the debounce queue.
    let queue_for_project_toggle = tree_queue;
    let mut workspace_open_for_toggle = workspace_open;
    let project_note_open_for_toggle = project_note_open;
    let tree_repo_for_hydrate = tree_repo.clone();
    let on_toggle_project = use_callback(move |id: Uuid| {
        let now_open = workspace_open_for_toggle
            .read()
            .get(&id.to_string())
            .copied()
            .unwrap_or(false);
        let next = !now_open;
        workspace_open_for_toggle.with_mut(|m| {
            m.insert(id.to_string(), next);
        });
        queue_for_project_toggle
            .read()
            .enqueue(SCOPE_WORKSPACE, id.to_string(), next);

        // Lazily hydrate the project's note-tree state when first opened.
        if next {
            let mut po = project_note_open_for_toggle;
            if !po.read().contains_key(&id) {
                match tree_repo_for_hydrate.snapshot_for_scope(&scope_for_project(id)) {
                    Ok(snap) => {
                        po.with_mut(|m| {
                            m.insert(id, snap);
                        });
                    }
                    Err(e) => {
                        eprintln!("operon: snapshot_for_scope project:{id} failed: {e}")
                    }
                }
            }
        }
    });

    let note_repo_for_add_root = note_repo.clone();
    let on_add_root_note = use_callback(move |project_id: Uuid| {
        match note_repo_for_add_root.create(project_id, None, "") {
            Ok(n) => {
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(Some(n.id));
            }
            Err(e) => eprintln!("operon: create local_note failed: {e}"),
        }
    });

    // Plans-Phase-6-image-notes: Add image note via native file picker.
    // Reads the chosen file, writes via images::write_image, mints an
    // image-note row, and attaches blob_path. Lives entirely on the
    // desktop side because rfd is desktop-only.
    let note_repo_for_add_image = note_repo.clone();
    let crate::local_mode::CurrentVaultRoot(vault_root_signal) = use_context();
    let on_add_image_note = use_callback(move |project_id: Uuid| {
        let Some(vault) = vault_root_signal.read().clone() else {
            eprintln!("operon: add image note: no vault");
            return;
        };
        let note_repo = note_repo_for_add_image.clone();
        spawn(async move {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title("Choose an image")
                .add_filter("Image", &["png", "jpg", "jpeg", "webp", "gif", "svg", "avif"])
                .pick_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("operon: read image file {path:?} failed: {e}");
                    return;
                }
            };
            // Cheap MIME inference from the chosen extension.
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            let mime = match ext.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "svg" => "image/svg+xml",
                "avif" => "image/avif",
                _ => {
                    eprintln!("operon: add image note: unsupported extension {ext}");
                    return;
                }
            };
            let written = match crate::local_mode::images::write_image(&vault, &bytes, mime) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("operon: write_image failed: {e}");
                    return;
                }
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Image")
                .to_string();
            match note_repo.create_with_kind(project_id, None, &stem, NoteKind::Image) {
                Ok(row) => {
                    let rel = written.relative_path.to_string_lossy().to_string();
                    if let Err(e) = note_repo.set_blob_path(row.id, Some(&rel)) {
                        eprintln!("operon: set_blob_path failed: {e}");
                    }
                    note_version.with_mut(|v| *v += 1);
                }
                Err(e) => eprintln!("operon: create image note failed: {e}"),
            }
        });
    });

    // ===== Note handlers =====
    let mut tabs_for_select = tabs;
    let scheduler_for_select = save_scheduler.clone();
    let on_select_note = use_callback(move |note_id: Uuid| {
        selected_note.set(Some(note_id));
        // Find note metadata to get the title; fall back to the id.
        let title = notes_by_project
            .read()
            .values()
            .flat_map(|list| list.iter())
            .find(|n| n.id == note_id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| note_id.to_string());
        let _ = open_local_note_tab(
            tabs_for_select,
            scheduler_for_select.clone(),
            note_id,
            title,
            String::new(),
        );
        let _ = tabs_for_select.write();
    });

    // Plans-Phase-5-vfs-wikilinks: rename propagation.
    let note_repo_for_rename = note_repo.clone();
    let project_repo_for_rename = project_repo.clone();
    let crate::local_mode::desktop::LocalNoteLinkRepo(link_repo_for_rename) = use_context();
    let persistence_for_rename: Arc<dyn Persistence> = use_context();
    let on_rename_note = use_callback(move |(id, new_title): (Uuid, String)| {
        if new_title.trim().is_empty() {
            renaming_note_setter.set(None);
            return;
        }
        // Capture the prior title (and the prior project name, for the
        // absolute-form rewrite) before the rename lands.
        let snap = notes_by_project.read();
        let mut old_title: Option<String> = None;
        let mut project_id_opt: Option<Uuid> = None;
        for (pid, list) in snap.iter() {
            if let Some(n) = list.iter().find(|n| n.id == id) {
                old_title = Some(n.title.clone());
                project_id_opt = Some(*pid);
                break;
            }
        }
        drop(snap);
        let project_name = project_id_opt
            .and_then(|pid| project_repo_for_rename.list().ok().and_then(|ps| {
                ps.into_iter().find(|p| p.id == pid).map(|p| p.name)
            }));

        match note_repo_for_rename.rename(id, &new_title) {
            Ok(()) => {
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(None);

                // Walk every referrer and rewrite `[[OldTitle]]` /
                // `[[Project/OldTitle]]` to the new equivalents in the
                // body, then patch local_note_link.target_text. Async via
                // `spawn` so the rename callback returns immediately.
                if let (Some(old), Some(proj_name)) = (old_title, project_name.clone()) {
                    if old != new_title {
                        let link_repo = link_repo_for_rename.clone();
                        let persistence = persistence_for_rename.clone();
                        let new_title_owned = new_title.clone();
                        spawn(async move {
                            let referrers = link_repo.referrers_of(id).unwrap_or_default();
                            for source in referrers {
                                let source_str = source.to_string();
                                let body_bytes = match persistence.load(&source_str).await {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };
                                let Ok(body) = String::from_utf8(body_bytes) else { continue };
                                let mut next = body.clone();
                                next = next.replace(
                                    &format!("[[{old}]]"),
                                    &format!("[[{new_title_owned}]]"),
                                );
                                next = next.replace(
                                    &format!("![[{old}]]"),
                                    &format!("![[{new_title_owned}]]"),
                                );
                                let old_abs = format!("[[{proj_name}/{old}]]");
                                let new_abs =
                                    format!("[[{proj_name}/{new_title_owned}]]");
                                next = next.replace(&old_abs, &new_abs);
                                let old_abs_embed = format!("![[{proj_name}/{old}]]");
                                let new_abs_embed =
                                    format!("![[{proj_name}/{new_title_owned}]]");
                                next = next.replace(&old_abs_embed, &new_abs_embed);
                                if next != body {
                                    let _ = persistence
                                        .save(&source_str, next.as_bytes())
                                        .await;
                                    let _ = link_repo.rewrite_target_text(
                                        id,
                                        &old,
                                        &new_title_owned,
                                    );
                                    let _ = link_repo.rewrite_target_text(
                                        id,
                                        &format!("{proj_name}/{old}"),
                                        &format!("{proj_name}/{new_title_owned}"),
                                    );
                                }
                            }
                        });
                    }
                }
            }
            Err(e) => {
                eprintln!("operon: rename local_note failed: {e}");
                renaming_note_setter.set(None);
            }
        }
    });

    let on_request_rename_note = use_callback(move |id: Uuid| {
        renaming_note_setter.set(Some(id));
    });
    let on_request_delete_note = use_callback(move |id: Uuid| {
        pending_delete_note_setter.set(Some(id));
    });

    // Plans-Phase-3-note-id-create: auto-expand the parent (and any collapsed
    // ancestors) so the new child/sibling rename input is visible without
    // manual chevron clicks. Walks the parent_id chain in `notes_by_project`
    // for the given project, marks each ancestor open in both the in-memory
    // `project_note_open` map and the persisted `tree_state` queue.
    let queue_for_expand = tree_queue;
    let mut project_note_open_for_expand = project_note_open;
    let notes_by_project_for_expand = notes_by_project;
    let expand_ancestors = move |project_id: Uuid, mut cursor: Option<Uuid>| {
        let scope = scope_for_project(project_id);
        let snap = notes_by_project_for_expand.read();
        let list = match snap.get(&project_id) {
            Some(l) => l,
            None => return,
        };
        while let Some(id) = cursor {
            let key = id.to_string();
            project_note_open_for_expand.with_mut(|map| {
                map.entry(project_id).or_default().insert(key.clone(), true);
            });
            queue_for_expand.read().enqueue(scope.clone(), key, true);
            cursor = list.iter().find(|n| n.id == id).and_then(|n| n.parent_id);
        }
    };

    let note_repo_for_add_child = note_repo.clone();
    let mut expand_ancestors_for_child = expand_ancestors.clone();
    let on_add_child_note = use_callback(move |parent_id: Uuid| {
        // Find parent's project_id.
        let project_id = notes_by_project
            .read()
            .iter()
            .find_map(|(pid, list)| list.iter().find(|n| n.id == parent_id).map(|_| *pid));
        let Some(project_id) = project_id else {
            eprintln!("operon: add child note: parent {parent_id} not found");
            return;
        };
        // Expand the parent and any collapsed ancestors before the create so
        // the inline rename input on the new row is visible.
        expand_ancestors_for_child(project_id, Some(parent_id));
        match note_repo_for_add_child.create(project_id, Some(parent_id), "") {
            Ok(n) => {
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(Some(n.id));
            }
            Err(e) => eprintln!("operon: create child note failed: {e}"),
        }
    });

    // Plans-Phase-3-note-id-create: insert a new sibling note immediately
    // after the target. Creates with the same `parent_id` as the target,
    // then `move_to` to land at `target.sibling_index + 1` (`move_to`
    // shifts the dense ordering). Triggers inline rename on the new row
    // and expands ancestors so the new row is visible.
    let note_repo_for_add_sibling = note_repo.clone();
    let mut expand_ancestors_for_sibling = expand_ancestors.clone();
    let on_add_sibling_note = use_callback(move |target_id: Uuid| {
        let snapshot = notes_by_project.read();
        let mut found: Option<(Uuid, Option<Uuid>, i64)> = None;
        for (pid, list) in snapshot.iter() {
            if let Some(target) = list.iter().find(|n| n.id == target_id) {
                found = Some((*pid, target.parent_id, target.sibling_index));
                break;
            }
        }
        drop(snapshot);
        let Some((project_id, parent_id, target_idx)) = found else {
            eprintln!("operon: add sibling: target {target_id} not found");
            return;
        };
        expand_ancestors_for_sibling(project_id, parent_id);
        match note_repo_for_add_sibling.create(project_id, parent_id, "") {
            Ok(n) => {
                if let Err(e) =
                    note_repo_for_add_sibling.move_to(n.id, project_id, parent_id, target_idx + 1)
                {
                    eprintln!("operon: add sibling: move_to failed: {e}");
                }
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(Some(n.id));
            }
            Err(e) => eprintln!("operon: create sibling note failed: {e}"),
        }
    });

    // ===== Phase-4 handlers: indent/outdent/move/clipboard =====
    let note_repo_for_indent = note_repo.clone();
    let on_indent_note = use_callback(move |id: Uuid| match note_repo_for_indent.indent(id) {
        Ok(()) => note_version.with_mut(|v| *v += 1),
        Err(e) => eprintln!("operon: indent note failed: {e}"),
    });
    let note_repo_for_outdent = note_repo.clone();
    let on_outdent_note = use_callback(move |id: Uuid| match note_repo_for_outdent.outdent(id) {
        Ok(()) => note_version.with_mut(|v| *v += 1),
        Err(e) => eprintln!("operon: outdent note failed: {e}"),
    });
    let note_repo_for_up = note_repo.clone();
    let on_move_up_note = use_callback(move |id: Uuid| match note_repo_for_up.move_up(id) {
        Ok(()) => note_version.with_mut(|v| *v += 1),
        Err(e) => eprintln!("operon: move_up note failed: {e}"),
    });
    let note_repo_for_down = note_repo.clone();
    let on_move_down_note = use_callback(move |id: Uuid| match note_repo_for_down.move_down(id) {
        Ok(()) => note_version.with_mut(|v| *v += 1),
        Err(e) => eprintln!("operon: move_down note failed: {e}"),
    });

    // Plans-Phase-6-image-notes: external image-file drop on a note row.
    // Writes the bytes to the vault, mints a child image-note under the
    // target row, and stamps blob_path. Mime is derived from the
    // filename's extension (caller pre-filtered to image extensions).
    let note_repo_for_drop_image = note_repo.clone();
    let crate::local_mode::CurrentVaultRoot(vault_for_drop_image) = use_context();
    let on_drop_image_into_note =
        use_callback(move |(parent_id, bytes, name): (Uuid, Vec<u8>, String)| {
            let Some(vault) = vault_for_drop_image.read().clone() else {
                eprintln!("operon: image drop: no vault");
                return;
            };
            let project_id_opt = {
                let snap = notes_by_project.read();
                snap.iter()
                    .find_map(|(pid, list)| {
                        list.iter().find(|n| n.id == parent_id).map(|_| *pid)
                    })
            };
            let Some(project_id) = project_id_opt else {
                eprintln!("operon: image drop: parent {parent_id} not found");
                return;
            };
            handle_image_drop(
                project_id,
                Some(parent_id),
                bytes,
                name,
                &vault,
                &note_repo_for_drop_image,
                note_version,
            );
        });
    let note_repo_for_drop_proj_image = note_repo.clone();
    let vault_for_drop_proj_image = vault_for_drop_image;
    let on_drop_image_into_project =
        use_callback(move |(project_id, bytes, name): (Uuid, Vec<u8>, String)| {
            let Some(vault) = vault_for_drop_proj_image.read().clone() else {
                eprintln!("operon: image drop: no vault");
                return;
            };
            handle_image_drop(
                project_id,
                None,
                bytes,
                name,
                &vault,
                &note_repo_for_drop_proj_image,
                note_version,
            );
        });

    // Plans-Phase-4-multiselect-aria: bulk delete every NodeKey in the
    // multi-selection set, in a single sweep through the repo. FK cascade
    // handles descendants. Project deletes are intentionally a no-op here
    // (the LocalProjectRepository doesn't expose delete; project removal
    // happens through a separate confirmation flow that's still
    // single-target).
    let note_repo_for_bulk_delete = note_repo.clone();
    let project_repo_for_bulk_gc = project_repo.clone();
    let note_repo_for_bulk_gc = note_repo.clone();
    let crate::local_mode::CurrentVaultRoot(vault_root_for_bulk_gc) = use_context();
    let on_confirm_bulk_delete = use_callback(move |_: ()| {
        let snapshot = multi_selected.read().clone();
        // Plans-Phase-6-image-notes: snapshot blob_paths to potentially
        // GC. We collect every blob_path of the targets + any descendants
        // before the delete tx fires (FK cascade loses them after).
        let mut blobs: Vec<String> = Vec::new();
        let snap = notes_by_project.read();
        let target_ids: std::collections::HashSet<Uuid> = snapshot
            .iter()
            .filter_map(|k| match k {
                NodeKey::Note(id) => Some(*id),
                NodeKey::Project(_) => None,
            })
            .collect();
        for list in snap.values() {
            for n in list.iter() {
                let touched = target_ids.contains(&n.id) || {
                    let mut cur = n.parent_id;
                    let mut hit = false;
                    while let Some(pid) = cur {
                        if target_ids.contains(&pid) {
                            hit = true;
                            break;
                        }
                        cur = list.iter().find(|x| x.id == pid).and_then(|x| x.parent_id);
                    }
                    hit
                };
                if touched {
                    if let Some(p) = n.blob_path.clone() {
                        blobs.push(p);
                    }
                }
            }
        }
        drop(snap);

        let mut deleted: usize = 0;
        for key in snapshot.iter() {
            if let NodeKey::Note(id) = key {
                match note_repo_for_bulk_delete.delete(*id) {
                    Ok(()) => deleted += 1,
                    Err(e) => eprintln!("operon: bulk delete note {id} failed: {e}"),
                }
            }
        }
        if deleted > 0 {
            note_version.with_mut(|v| *v += 1);
            if let Some(vault) = vault_root_for_bulk_gc.read().clone() {
                let projects = project_repo_for_bulk_gc.list().unwrap_or_default();
                for blob in blobs {
                    let mut still_referenced = false;
                    'outer: for p in &projects {
                        if let Ok(notes) = note_repo_for_bulk_gc.list_for_project(p.id) {
                            for n in notes {
                                if n.blob_path.as_deref() == Some(blob.as_str()) {
                                    still_referenced = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                    if !still_referenced {
                        let _ = std::fs::remove_file(vault.path().join(&blob));
                    }
                }
            }
        }
        multi_selected.set(std::collections::BTreeSet::new());
        pending_bulk_delete_setter.set(false);
    });
    let on_cancel_bulk_delete = use_callback(move |_: ()| {
        pending_bulk_delete_setter.set(false);
    });

    // Plans-Phase-4-multiselect-aria: bulk export. Opens a native folder
    // picker, then writes each selected note's body as `<title>.md` in
    // that directory. Image notes copy their blob next to the markdown
    // file. Title collisions get a numeric suffix.
    let project_repo_for_export = project_repo.clone();
    let note_repo_for_export = note_repo.clone();
    let project_repo_for_gc = project_repo.clone();
    let note_repo_for_gc = note_repo.clone();
    let persistence_for_export: Arc<dyn Persistence> = use_context();
    let crate::local_mode::CurrentVaultRoot(vault_root_for_export) = use_context();
    let on_bulk_export = use_callback(move |_: ()| {
        let snapshot = multi_selected_for_render.read().clone();
        if snapshot.is_empty() {
            return;
        }
        let project_repo = project_repo_for_export.clone();
        let note_repo = note_repo_for_export.clone();
        let persistence = persistence_for_export.clone();
        let vault = vault_root_for_export.read().clone();
        spawn(async move {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title("Export selection to folder")
                .pick_folder()
                .await
            else {
                return;
            };
            let target = handle.path().to_path_buf();
            let _ = std::fs::create_dir_all(&target);
            let projects = project_repo.list().unwrap_or_default();
            // Index notes by id for cheap lookup.
            let mut by_id: std::collections::HashMap<Uuid, operon_store::repos::LocalNote> =
                std::collections::HashMap::new();
            for p in &projects {
                if let Ok(notes) = note_repo.list_for_project(p.id) {
                    for n in notes {
                        by_id.insert(n.id, n);
                    }
                }
            }
            let mut written: usize = 0;
            for key in snapshot.iter() {
                let NodeKey::Note(id) = key else {
                    continue;
                };
                let Some(note) = by_id.get(id).cloned() else {
                    continue;
                };
                let safe_title = sanitize_filename(&note.title);
                match note.kind {
                    operon_store::repos::NoteKind::Markdown => {
                        let body = match persistence.load(&id.to_string()).await {
                            Ok(b) => b,
                            Err(_) => Vec::new(),
                        };
                        let path = unique_path(&target, &safe_title, "md");
                        if std::fs::write(&path, &body).is_ok() {
                            written += 1;
                        }
                    }
                    operon_store::repos::NoteKind::Image => {
                        let Some(rel) = note.blob_path.clone() else { continue };
                        let Some(vr) = vault.clone() else { continue };
                        let bytes = match crate::local_mode::images::read_image(
                            &vr,
                            std::path::Path::new(&rel),
                        ) {
                            Ok(b) => b,
                            Err(_) => continue,
                        };
                        let ext = std::path::Path::new(&rel)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("png");
                        let path = unique_path(&target, &safe_title, ext);
                        if std::fs::write(&path, &bytes).is_ok() {
                            written += 1;
                        }
                    }
                }
            }
            eprintln!("operon: bulk export wrote {written} file(s) to {target:?}");
        });
    });

    // Right-click → View / Edit / Split-view: switch the editor mode of the
    // note's tab, opening it first if it isn't already open.
    let mut tabs_for_mode = tabs;
    let scheduler_for_mode = save_scheduler.clone();
    let on_set_note_mode = use_callback(move |(note_id, target): (Uuid, EditorMode)| {
        let existing_tab_id = tabs_for_mode
            .read()
            .iter()
            .find(|t| t.note_id == note_id.to_string())
            .map(|t| t.id);
        let tab_id = if let Some(id) = existing_tab_id {
            id
        } else {
            let title = notes_by_project
                .read()
                .values()
                .flat_map(|list| list.iter())
                .find(|n| n.id == note_id)
                .map(|n| n.title.clone())
                .unwrap_or_else(|| note_id.to_string());
            open_local_note_tab(
                tabs_for_mode,
                scheduler_for_mode.clone(),
                note_id,
                title,
                String::new(),
            )
        };
        tabs_for_mode.write().set_mode(tab_id, target);
        selected_note.set(Some(note_id));
    });

    // Clipboard helpers shared by row context-menu items.
    let mut clipboard_for_cut = clipboard;
    let on_cut_note = use_callback(move |id: Uuid| {
        clipboard_for_cut.set(Some(Clipboard::cut_note(id)));
    });
    let mut clipboard_for_copy = clipboard;
    let on_copy_note = use_callback(move |id: Uuid| {
        clipboard_for_copy.set(Some(Clipboard::copy_note(id)));
    });
    let mut clipboard_for_cut_p = clipboard;
    let on_cut_project = use_callback(move |id: Uuid| {
        clipboard_for_cut_p.set(Some(Clipboard::cut_project(id)));
    });
    let mut clipboard_for_copy_p = clipboard;
    let on_copy_project = use_callback(move |id: Uuid| {
        clipboard_for_copy_p.set(Some(Clipboard::copy_project(id)));
    });

    // Paste from row context menu (target = row id). Selected note → child;
    // selected project → root.
    let note_repo_for_paste = note_repo.clone();
    let mut clipboard_for_paste = clipboard;
    let on_paste_into_note = use_callback(move |target: Uuid| {
        let Some(clip) = *clipboard_for_paste.read() else {
            return;
        };
        // Locate the destination project via the target note.
        let project_id = notes_by_project
            .read()
            .iter()
            .find_map(|(pid, list)| list.iter().find(|n| n.id == target).map(|_| *pid));
        let Some(project_id) = project_id else {
            return;
        };
        let last_index = note_repo_for_paste
            .list_for_project(project_id)
            .map(|rows| rows.iter().filter(|r| r.parent_id == Some(target)).count() as i64)
            .unwrap_or(0);
        let outcome = match (clip.kind, clip.payload) {
            (ClipKind::Cut, ClipPayload::Note(nid)) => {
                note_repo_for_paste.move_to(nid, project_id, Some(target), last_index)
            }
            (ClipKind::Copy, ClipPayload::Note(nid)) => note_repo_for_paste
                .duplicate_subtree(nid, project_id, Some(target), last_index)
                .map(|_| ()),
            (_, ClipPayload::Project(_)) => Ok(()),
        };
        if let Err(e) = outcome {
            eprintln!("operon: paste-into-note failed: {e}");
            return;
        }
        note_version.with_mut(|v| *v += 1);
        if matches!(clip.kind, ClipKind::Cut) {
            clipboard_for_paste.set(None);
        }
    });

    let note_repo_for_paste_proj = note_repo.clone();
    let mut clipboard_for_paste_proj = clipboard;
    let on_paste_into_project = use_callback(move |target_project: Uuid| {
        let Some(clip) = *clipboard_for_paste_proj.read() else {
            return;
        };
        let last_index = note_repo_for_paste_proj
            .list_for_project(target_project)
            .map(|rows| rows.iter().filter(|r| r.parent_id.is_none()).count() as i64)
            .unwrap_or(0);
        let outcome = match (clip.kind, clip.payload) {
            (ClipKind::Cut, ClipPayload::Note(nid)) => {
                note_repo_for_paste_proj.move_to(nid, target_project, None, last_index)
            }
            (ClipKind::Copy, ClipPayload::Note(nid)) => note_repo_for_paste_proj
                .duplicate_subtree(nid, target_project, None, last_index)
                .map(|_| ()),
            (_, ClipPayload::Project(_)) => Ok(()),
        };
        if let Err(e) = outcome {
            eprintln!("operon: paste-into-project failed: {e}");
            return;
        }
        note_version.with_mut(|v| *v += 1);
        if matches!(clip.kind, ClipKind::Cut) {
            clipboard_for_paste_proj.set(None);
        }
    });

    // Drag-drop: reorder projects (Project ↔ Project).
    let project_repo_for_drop = project_repo.clone();
    let on_drop_project_on_project =
        use_callback(move |(src, target, pos): (Uuid, Uuid, DropPosition)| {
            if matches!(pos, DropPosition::Into) || src == target {
                return;
            }
            let projects_now = match project_repo_for_drop.list() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("operon: list projects (drop) failed: {e}");
                    return;
                }
            };
            let target_idx = match projects_now.iter().position(|p| p.id == target) {
                Some(i) => i as i64,
                None => return,
            };
            let new_idx = match pos {
                DropPosition::Before => target_idx,
                DropPosition::After => target_idx + 1,
                DropPosition::Into => return,
            };
            if let Err(e) = project_repo_for_drop.reorder(src, new_idx) {
                eprintln!("operon: reorder project failed: {e}");
                return;
            }
            project_version.with_mut(|v| *v += 1);
        });

    // Drag-drop: note onto project (Into = move to project root last; Before/After = no-op).
    let note_repo_for_drop_pr = note_repo.clone();
    let on_drop_note_on_project =
        use_callback(move |(src, target, pos): (Uuid, Uuid, DropPosition)| {
            if !matches!(pos, DropPosition::Into) {
                return;
            }
            let last_index = note_repo_for_drop_pr
                .list_for_project(target)
                .map(|rows| rows.iter().filter(|r| r.parent_id.is_none()).count() as i64)
                .unwrap_or(0);
            if let Err(e) = note_repo_for_drop_pr.move_to(src, target, None, last_index) {
                eprintln!("operon: drop note onto project failed: {e}");
                return;
            }
            note_version.with_mut(|v| *v += 1);
        });

    // Drag-drop: note onto note. Before/After → sibling at target.parent_id;
    // Into → child of target.
    let note_repo_for_drop_n = note_repo.clone();
    let on_drop_note_on_note =
        use_callback(move |(src, target, pos): (Uuid, Uuid, DropPosition)| {
            if src == target {
                return;
            }
            // Look up target's project + parent.
            let info = notes_by_project.read().iter().find_map(|(pid, list)| {
                list.iter()
                    .find(|n| n.id == target)
                    .map(|n| (*pid, n.parent_id, n.sibling_index))
            });
            let Some((project_id, target_parent, target_sibling)) = info else {
                return;
            };
            let outcome = match pos {
                DropPosition::Into => {
                    let last_index = note_repo_for_drop_n
                        .list_for_project(project_id)
                        .map(|rows| {
                            rows.iter().filter(|r| r.parent_id == Some(target)).count() as i64
                        })
                        .unwrap_or(0);
                    note_repo_for_drop_n.move_to(src, project_id, Some(target), last_index)
                }
                DropPosition::Before => {
                    note_repo_for_drop_n.move_to(src, project_id, target_parent, target_sibling)
                }
                DropPosition::After => {
                    note_repo_for_drop_n.move_to(src, project_id, target_parent, target_sibling + 1)
                }
            };
            if let Err(e) = outcome {
                eprintln!("operon: drop note onto note failed: {e}");
                return;
            }
            note_version.with_mut(|v| *v += 1);
        });

    // Toggle a note open/closed.
    let queue_for_note_toggle = tree_queue;
    let mut project_note_open_setter = project_note_open;
    let on_toggle_note = use_callback(move |(project_id, note_id): (Uuid, Uuid)| {
        let scope = scope_for_project(project_id);
        let now_open = project_note_open_setter
            .read()
            .get(&project_id)
            .and_then(|m| m.get(&note_id.to_string()).copied())
            .unwrap_or(false);
        let next = !now_open;
        project_note_open_setter.with_mut(|map| {
            map.entry(project_id)
                .or_default()
                .insert(note_id.to_string(), next);
        });
        queue_for_note_toggle
            .read()
            .enqueue(scope, note_id.to_string(), next);
    });

    // Plans-Phase-4-multiselect-aria: maintain a global visible-flat tree
    // signal so Shift+click can compute true ranges. Recomputed on every
    // ExplorerPanel render — Dioxus signals are cheap and the inputs
    // already drive a re-render of the tree.
    let visible_flat: Signal<Vec<NodeKey>> = use_context::<VisibleFlat>().0;
    let mut visible_flat_setter = visible_flat;
    use_effect(move || {
        let projects_snap = projects.read().clone();
        let workspace_snap = workspace_open.read().clone();
        let notes_snap = notes_by_project.read().clone();
        let open_snap = project_note_open.read().clone();
        let mut out = Vec::with_capacity(64);
        for p in &projects_snap {
            out.push(NodeKey::Project(p.id));
            let project_open =
                workspace_snap.get(&p.id.to_string()).copied().unwrap_or(false);
            if !project_open {
                continue;
            }
            let project_open_map = open_snap.get(&p.id).cloned().unwrap_or_default();
            let notes = notes_snap.get(&p.id).cloned().unwrap_or_default();
            let forest = NoteForest::from_flat(notes);
            let visible = flatten_visible(&forest, &|id: &Uuid| {
                project_open_map
                    .get(&id.to_string())
                    .copied()
                    .unwrap_or(false)
            });
            for n in visible {
                out.push(NodeKey::Note(n.id));
            }
        }
        visible_flat_setter.set(out);
    });

    // ===== Render =====
    let projects_snapshot = projects.read().clone();
    let renaming_project_now = *renaming_project.read();
    let renaming_note_now = *renaming_note.read();
    let selected_project_now = *selected_project.read();
    let selected_note_now = *selected_note.read();

    let pending_delete_project_id = *pending_delete_project.read();
    let pending_delete_project_name = pending_delete_project_id.and_then(|did| {
        projects_snapshot
            .iter()
            .find(|p| p.id == did)
            .map(|p| p.name.clone())
    });
    let pending_delete_note_id = *pending_delete_note.read();
    let pending_delete_note_title = pending_delete_note_id.and_then(|did| {
        notes_by_project
            .read()
            .values()
            .flat_map(|list| list.iter())
            .find(|n| n.id == did)
            .map(|n| n.title.clone())
    });

    let project_repo_for_delete = project_repo.clone();
    let note_repo_for_delete = note_repo.clone();
    // Snapshot the per-note current editor mode from the tab manager so each
    // NoteRow can show the right context-menu items (View / Edit / Split).
    let note_modes_snapshot: HashMap<Uuid, EditorMode> = {
        let snap = tabs.read();
        snap.iter()
            .filter_map(|t| Uuid::parse_str(&t.note_id).ok().map(|u| (u, t.mode)))
            .collect()
    };
    let workspace_snap = workspace_open.read().clone();
    let project_note_open_snap = project_note_open.read().clone();
    let notes_snap = notes_by_project.read().clone();
    let clipboard_snap = *clipboard.read();
    let drag_session_now = *drag_session.read();
    let has_clip_note = matches!(
        clipboard_snap,
        Some(Clipboard {
            payload: ClipPayload::Note(_),
            ..
        })
    );

    // Phase-5: title lookup for the search-result click handler.
    let note_titles_signal: Signal<HashMap<Uuid, String>> = use_signal(HashMap::new);
    {
        let mut titles_setter = note_titles_signal;
        use_effect(move || {
            let map = notes_by_project.read();
            let mut out: HashMap<Uuid, String> = HashMap::new();
            for list in map.values() {
                for n in list {
                    out.insert(n.id, n.title.clone());
                }
            }
            titles_setter.set(out);
        });
    }
    let on_search_pick = search_click_handler(
        tabs,
        save_scheduler.clone(),
        selected_note,
        selected_project,
        workspace_open,
        tree_queue,
        note_titles_signal,
        search_query,
        search_in_content,
    );

    // Esc inside the search input clears query + restores focus to prev_selection.
    let prev_selection_setter = prev_selection;
    let mut search_query_setter = search_query;
    let mut search_in_content_setter = search_in_content;
    let mut selected_note_for_clear = selected_note;
    let on_search_clear = use_callback(move |_| {
        search_query_setter.set(String::new());
        search_in_content_setter.set(false);
        if let Some(prev) = *prev_selection_setter.read() {
            selected_note_for_clear.set(Some(prev));
        }
    });

    // Capture prev_selection on transition from empty -> non-empty query.
    {
        let mut prev_setter = prev_selection;
        use_effect(move || {
            let q = search_query.read().clone();
            if !q.is_empty() && prev_setter.read().is_none() {
                prev_setter.set(*selected_note.read());
            } else if q.is_empty() {
                prev_setter.set(None);
            }
        });
    }

    let debounced_now = debounced_query.read().clone();
    let in_content_now = *search_in_content.read();
    let body_cache_now = body_cache.read().clone();
    let show_results = !debounced_now.trim().is_empty();

    rsx! {
        div {
            class: "notes-explorer-list",
            "data-testid": "explorer-panel",
            style: "list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column;",
            // Header — search input + checkbox + "+" button. Borderless to match
            // Cloud's notes-explorer chrome density.
            div {
                class: "notes-explorer-toolbar",
                div {
                    class: "notes-explorer-toolbar-search",
                    ExplorerSearch {
                        query: search_query,
                        in_content: search_in_content,
                        focus_tick: focus_tick,
                        on_clear: on_search_clear,
                    }
                }
                button {
                    r#type: "button",
                    class: "notes-explorer-toolbar-add",
                    "data-testid": "explorer-add-project",
                    "aria-label": "New project",
                    title: "New project",
                    onclick: on_add_project,
                    span { "aria-hidden": "true", "+" }
                    span { class: "sr-only", "New project" }
                }
            }
            // Body — results when query is non-empty, otherwise the tree.
            if show_results {
                ResultsList {
                    query: debounced_now.clone(),
                    in_content: in_content_now,
                    body_cache: body_cache_now.clone(),
                    on_pick: on_search_pick,
                }
            } else {
            div {
                class: "flex-1 overflow-y-auto",
                // Plans-Phase-4: WAI-ARIA tree pattern. Multi-select isn't
                // wired yet (single-selection signals still in place) so we
                // advertise aria-multiselectable=false for now; flipping it
                // is the BTreeSet<NodeKey> follow-up.
                role: "tree",
                "aria-multiselectable": "true",
                "aria-label": "Projects and notes",
                tabindex: "-1",
                onkeydown: move |evt| {
                    let key = evt.key().to_string();
                    let mods = evt.modifiers();
                    let with_meta = mods.contains(keyboard_types::Modifiers::META)
                        || mods.contains(keyboard_types::Modifiers::CONTROL);
                    if key == "Delete" || key == "Backspace" {
                        let count = multi_selected_for_render.read().len();
                        if count >= 2 {
                            evt.prevent_default();
                            pending_bulk_delete_setter.set(true);
                        }
                    } else if with_meta && mods.contains(keyboard_types::Modifiers::SHIFT)
                        && key.eq_ignore_ascii_case("e")
                    {
                        let count = multi_selected_for_render.read().len();
                        if count >= 1 {
                            evt.prevent_default();
                            on_bulk_export.call(());
                        }
                    } else if with_meta && mods.contains(keyboard_types::Modifiers::SHIFT)
                        && key.eq_ignore_ascii_case("r")
                    {
                        let count = multi_selected_for_render.read().len();
                        if count >= 1 {
                            evt.prevent_default();
                            pending_bulk_rename.set(true);
                        }
                    }
                },
                if projects_snapshot.is_empty() {
                    div {
                        class: "px-3 py-6 text-xs opacity-60 text-center",
                        "data-testid": "explorer-empty",
                        "No projects yet. Click + to create one."
                    }
                } else {
                    for project in projects_snapshot.iter().cloned() {
                        ProjectSubtree {
                            key: "{project.id}",
                            project: project.clone(),
                            is_open: workspace_snap
                                .get(&project.id.to_string())
                                .copied()
                                .unwrap_or(false),
                            selected: selected_project_now == Some(project.id),
                            in_rename: renaming_project_now == Some(project.id),
                            notes: notes_snap.get(&project.id).cloned().unwrap_or_default(),
                            note_open_snap: project_note_open_snap
                                .get(&project.id)
                                .cloned()
                                .unwrap_or_default(),
                            renaming_note: renaming_note_now,
                            selected_note: selected_note_now,
                            clipboard: clipboard_snap,
                            has_clip_note: has_clip_note,
                            drag_session: drag_session_now,
                            on_select_project: on_select_project,
                            on_rename_project: on_rename_project,
                            on_delete_project: on_delete_project_noop,
                            on_request_rename_project: on_request_rename_project,
                            on_request_delete_project: on_request_delete_project,
                            on_toggle_project: on_toggle_project,
                            on_add_root_note: on_add_root_note,
                            on_add_image_note: on_add_image_note,
                            on_drop_image_into_note: on_drop_image_into_note,
                            on_drop_image_into_project: on_drop_image_into_project,
                            on_select_note: on_select_note,
                            on_toggle_note: on_toggle_note,
                            on_rename_note: on_rename_note,
                            on_request_rename_note: on_request_rename_note,
                            on_request_delete_note: on_request_delete_note,
                            on_add_child_note: on_add_child_note,
                            on_add_sibling_note: on_add_sibling_note,
                            on_indent_note: on_indent_note,
                            on_outdent_note: on_outdent_note,
                            on_move_up_note: on_move_up_note,
                            on_move_down_note: on_move_down_note,
                            on_cut_note: on_cut_note,
                            on_copy_note: on_copy_note,
                            on_paste_into_note: on_paste_into_note,
                            on_cut_project: on_cut_project,
                            on_copy_project: on_copy_project,
                            on_paste_into_project: on_paste_into_project,
                            on_drop_project_on_project: on_drop_project_on_project,
                            on_drop_note_on_project: on_drop_note_on_project,
                            on_drop_note_on_note: on_drop_note_on_note,
                            note_modes: note_modes_snapshot.clone(),
                            on_set_note_mode: on_set_note_mode,
                        }
                    }
                }
            }
            }
        }
        if let Some(did) = pending_delete_project_id {
            ConfirmDialog {
                title: "Delete project".to_string(),
                message: format!(
                    "Delete project \"{}\"?\nThis cannot be undone.",
                    pending_delete_project_name.clone().unwrap_or_default()
                ),
                confirm_label: "Delete".to_string(),
                on_confirm: Callback::new(move |_| {
                    match project_repo_for_delete.delete(did) {
                        Ok(()) => {
                            project_version.with_mut(|v| *v += 1);
                            note_version.with_mut(|v| *v += 1);
                            if selected_project_now == Some(did) {
                                selected_project.set(None);
                            }
                        }
                        Err(e) => eprintln!("operon: delete local_project failed: {e}"),
                    }
                    pending_delete_project_setter.set(None);
                }),
                on_cancel: Callback::new(move |_| {
                    pending_delete_project_setter.set(None);
                }),
            }
        }
        if let Some(did) = pending_delete_note_id {
            ConfirmDialog {
                title: "Delete note".to_string(),
                message: format!(
                    "Delete note \"{}\"?\nChild notes are deleted too.\nThis cannot be undone.",
                    pending_delete_note_title.clone().unwrap_or_default()
                ),
                confirm_label: "Delete".to_string(),
                on_confirm: Callback::new(move |_| {
                    // Plans-Phase-6-image-notes: snapshot the note's blob_path
                    // (and the blob_paths of any descendants) BEFORE delete,
                    // since the FK cascade will lose them.
                    let mut blobs_to_check: Vec<String> = Vec::new();
                    let snap = notes_by_project.read();
                    for list in snap.values() {
                        // Collect the target plus all descendants whose
                        // ancestor chain includes `did`.
                        for n in list.iter() {
                            if n.id == did {
                                if let Some(ref p) = n.blob_path {
                                    blobs_to_check.push(p.clone());
                                }
                            }
                            // Walk ancestors of n; if any ancestor is `did`,
                            // n is being deleted too.
                            let mut cur = n.parent_id;
                            while let Some(pid) = cur {
                                if pid == did {
                                    if let Some(ref p) = n.blob_path {
                                        blobs_to_check.push(p.clone());
                                    }
                                    break;
                                }
                                cur = list.iter().find(|x| x.id == pid).and_then(|x| x.parent_id);
                            }
                        }
                    }
                    drop(snap);
                    match note_repo_for_delete.delete(did) {
                        Ok(()) => {
                            note_version.with_mut(|v| *v += 1);
                            if selected_note_now == Some(did) {
                                selected_note.set(None);
                            }
                            // Refcount each blob: if no remaining note
                            // references it, delete the on-disk file.
                            if let Some(vault) = vault_root_for_export.read().clone() {
                                let project_repo = project_repo_for_gc.clone();
                                let note_repo = note_repo_for_gc.clone();
                                let projects = project_repo.list().unwrap_or_default();
                                for blob in blobs_to_check {
                                    let mut still_referenced = false;
                                    'outer: for p in &projects {
                                        if let Ok(notes) = note_repo.list_for_project(p.id) {
                                            for n in notes {
                                                if n.blob_path.as_deref() == Some(blob.as_str()) {
                                                    still_referenced = true;
                                                    break 'outer;
                                                }
                                            }
                                        }
                                    }
                                    if !still_referenced {
                                        let abs = vault.path().join(&blob);
                                        let _ = std::fs::remove_file(&abs);
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("operon: delete local_note failed: {e}"),
                    }
                    pending_delete_note_setter.set(None);
                }),
                on_cancel: Callback::new(move |_| {
                    pending_delete_note_setter.set(None);
                }),
            }
        }
        // Plans-Phase-4-multiselect-aria: bulk rename modal. Cmd/Ctrl+Shift+R
        // when the multi-selection set has 1+ items. Apply re-bumps
        // note_version so the explorer refreshes.
        if *pending_bulk_rename.read() {
            BulkRenameModal {
                open: pending_bulk_rename,
                on_applied: move |_count: usize| {
                    note_version.with_mut(|v| *v += 1);
                },
            }
        }
        // Plans-Phase-4-multiselect-aria: bulk-delete confirmation. Triggered
        // by Delete/Backspace when the multi-selection set has 2+ items.
        if *pending_bulk_delete.read() {
            {
                let snap = multi_selected_for_render.read();
                let note_count = snap.iter().filter(|k| matches!(k, NodeKey::Note(_))).count();
                let project_count = snap.iter().filter(|k| matches!(k, NodeKey::Project(_))).count();
                let total = snap.len();
                let breakdown = if project_count > 0 {
                    format!(
                        "{} item(s) selected — {} note(s), {} project(s).\nProjects in the set are kept (use Delete project for those). Notes will be deleted with their children.\nThis cannot be undone.",
                        total, note_count, project_count
                    )
                } else {
                    format!(
                        "{} note(s) selected. Child notes are deleted too.\nThis cannot be undone.",
                        note_count
                    )
                };
                drop(snap);
                rsx! {
                    ConfirmDialog {
                        title: "Bulk delete".to_string(),
                        message: breakdown,
                        confirm_label: "Delete all".to_string(),
                        on_confirm: Callback::new(move |_| {
                            on_confirm_bulk_delete.call(());
                        }),
                        on_cancel: Callback::new(move |_| {
                            on_cancel_bulk_delete.call(());
                        }),
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ProjectSubtreeProps {
    project: LocalProject,
    is_open: bool,
    selected: bool,
    in_rename: bool,
    notes: Vec<LocalNote>,
    note_open_snap: HashMap<String, bool>,
    renaming_note: Option<Uuid>,
    selected_note: Option<Uuid>,
    clipboard: Option<Clipboard>,
    has_clip_note: bool,
    drag_session: Option<DragKind>,
    on_select_project: Callback<Uuid>,
    on_rename_project: Callback<(Uuid, String)>,
    on_delete_project: Callback<Uuid>,
    on_request_rename_project: Callback<Uuid>,
    on_request_delete_project: Callback<Uuid>,
    on_toggle_project: Callback<Uuid>,
    on_add_root_note: Callback<Uuid>,
    on_add_image_note: Callback<Uuid>,
    on_drop_image_into_note: Callback<(Uuid, Vec<u8>, String)>,
    on_drop_image_into_project: Callback<(Uuid, Vec<u8>, String)>,
    on_select_note: Callback<Uuid>,
    on_toggle_note: Callback<(Uuid, Uuid)>,
    on_rename_note: Callback<(Uuid, String)>,
    on_request_rename_note: Callback<Uuid>,
    on_request_delete_note: Callback<Uuid>,
    on_add_child_note: Callback<Uuid>,
    on_add_sibling_note: Callback<Uuid>,
    on_indent_note: Callback<Uuid>,
    on_outdent_note: Callback<Uuid>,
    on_move_up_note: Callback<Uuid>,
    on_move_down_note: Callback<Uuid>,
    on_cut_note: Callback<Uuid>,
    on_copy_note: Callback<Uuid>,
    on_paste_into_note: Callback<Uuid>,
    on_cut_project: Callback<Uuid>,
    on_copy_project: Callback<Uuid>,
    on_paste_into_project: Callback<Uuid>,
    on_drop_project_on_project: Callback<(Uuid, Uuid, DropPosition)>,
    on_drop_note_on_project: Callback<(Uuid, Uuid, DropPosition)>,
    on_drop_note_on_note: Callback<(Uuid, Uuid, DropPosition)>,
    note_modes: HashMap<Uuid, EditorMode>,
    on_set_note_mode: Callback<(Uuid, EditorMode)>,
}

#[component]
fn ProjectSubtree(props: ProjectSubtreeProps) -> Element {
    let project_id = props.project.id;
    let forest = NoteForest::from_flat(props.notes.clone());
    let visible = if props.is_open {
        flatten_visible(&forest, &|id: &Uuid| {
            props
                .note_open_snap
                .get(&id.to_string())
                .copied()
                .unwrap_or(false)
        })
    } else {
        Vec::new()
    };

    // Build a sibling-index map: (parent, sibling_index, max_sibling_index) for
    // each visible note. Used by NoteRow to enable/disable indent / move-up / etc.
    let mut max_sibling_by_parent: HashMap<Option<Uuid>, i64> = HashMap::new();
    for note in props.notes.iter() {
        let entry = max_sibling_by_parent
            .entry(note.parent_id)
            .or_insert(note.sibling_index);
        if note.sibling_index > *entry {
            *entry = note.sibling_index;
        }
    }

    let on_toggle_note = props.on_toggle_note;
    let toggle_note_for_project: Callback<Uuid> = use_callback(move |note_id: Uuid| {
        on_toggle_note.call((project_id, note_id));
    });

    let cut_project = props
        .clipboard
        .map(|c| c.is_cut_project(project_id))
        .unwrap_or(false);

    rsx! {
        ProjectRow {
            project: props.project.clone(),
            is_open: props.is_open,
            selected: props.selected,
            in_rename: props.in_rename,
            cut: cut_project,
            has_clip_note: props.has_clip_note,
            drag_active: props.drag_session.is_some(),
            on_select: props.on_select_project,
            on_rename: props.on_rename_project,
            on_delete: props.on_delete_project,
            on_request_rename: props.on_request_rename_project,
            on_request_delete: props.on_request_delete_project,
            on_toggle: props.on_toggle_project,
            on_add_note: props.on_add_root_note,
            on_add_image_note: props.on_add_image_note,
            on_drop_image_file: props.on_drop_image_into_project,
            on_cut: props.on_cut_project,
            on_copy: props.on_copy_project,
            on_paste: props.on_paste_into_project,
            on_drop_project_on_project: props.on_drop_project_on_project,
            on_drop_note_on_project: props.on_drop_note_on_project,
        }
        for note in visible.into_iter() {
            {
                let max_sibling = max_sibling_by_parent
                    .get(&note.parent_id)
                    .copied()
                    .unwrap_or(0);
                let is_first = note.sibling_index == 0;
                let is_last = note.sibling_index == max_sibling;
                let depth = note.depth;
                let cut = props
                    .clipboard
                    .map(|c| c.is_cut_note(note.id))
                    .unwrap_or(false);
                rsx! {
                    NoteRow {
                        key: "{note.id}",
                        note: note.clone(),
                        depth,
                        has_children: forest.has_children(&note.id),
                        is_open: props
                            .note_open_snap
                            .get(&note.id.to_string())
                            .copied()
                            .unwrap_or(false),
                        selected: props.selected_note == Some(note.id),
                        in_rename: props.renaming_note == Some(note.id),
                        is_first_sibling: is_first,
                        is_last_sibling: is_last,
                        cut,
                        has_clip_note: props.has_clip_note,
                        drag_active: props.drag_session.is_some(),
                        on_select: props.on_select_note,
                        on_toggle_open: toggle_note_for_project,
                        on_rename: props.on_rename_note,
                        on_request_rename: props.on_request_rename_note,
                        on_request_delete: props.on_request_delete_note,
                        on_add_child: props.on_add_child_note,
                        on_add_sibling: props.on_add_sibling_note,
                        on_drop_image_file: props.on_drop_image_into_note,
                        on_indent: props.on_indent_note,
                        on_outdent: props.on_outdent_note,
                        on_move_up: props.on_move_up_note,
                        on_move_down: props.on_move_down_note,
                        on_cut: props.on_cut_note,
                        on_copy: props.on_copy_note,
                        on_paste: props.on_paste_into_note,
                        on_drop_note_on_note: props.on_drop_note_on_note,
                        current_mode: props.note_modes.get(&note.id).copied(),
                        on_set_mode: props.on_set_note_mode,
                    }
                }
            }
        }
    }
}
