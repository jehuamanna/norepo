//! Local-Mode explorer panel: lists `local_project` rows with rename/delete and
//! a "+" button to create a new (default-named) project. Phase 3 nests notes
//! under each project, persisted via `local_note` + `local_tree_state`.

mod backlinks;
mod bulk_rename;
pub mod creatable_kind;
pub mod history;
mod note_row;
mod project_row;
mod role;
mod search;
mod tree_node;
mod tree_state;

pub use backlinks::BacklinksPanel;
pub use bulk_rename::BulkRenameModal;

pub use note_row::NoteRow;
pub use project_row::ProjectRow;
pub use search::{
    click_handler as search_click_handler, debounce_window, load_body_cache, BodyCache,
    ExplorerSearch, ExplorerSearchRepo, ResultsList,
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
use crate::local_mode::explorer::creatable_kind::CreatableKind;
use crate::local_mode::ui::{
    resolve_drop_parent, ClipKind, ClipPayload, Clipboard, ConfirmDialog, DragKind, DragSession,
    DropPosition, LocalClipboard,
};
use crate::persistence::{PersistError, Persistence};
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

/// Sibling-group classification for the bulk drag/drop guard. Two
/// `NodeKey`s are siblings iff they map to the same `SiblingGroup`.
/// Projects collectively share the workspace root; notes share whatever
/// `parent_id` they sit directly under (regardless of project).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SiblingGroup {
    WorkspaceRoot,
    UnderParent(Option<Uuid>),
}

fn classify_sibling_group(
    key: &NodeKey,
    notes_by_project: &HashMap<Uuid, Vec<LocalNote>>,
) -> Option<SiblingGroup> {
    match key {
        NodeKey::Project(_) => Some(SiblingGroup::WorkspaceRoot),
        NodeKey::Note(id) => notes_by_project
            .values()
            .flat_map(|v| v.iter())
            .find(|n| &n.id == id)
            .map(|n| SiblingGroup::UnderParent(n.parent_id)),
    }
}

/// True iff every member of `set` resolves to the same [`SiblingGroup`].
/// An empty / singleton set is trivially "all siblings". Members not
/// found in `notes_by_project` are skipped (a missing note is treated as
/// neither violating nor satisfying the rule — the caller's drop logic
/// already rejects unresolved sources).
pub(super) fn all_siblings(
    set: &std::collections::BTreeSet<NodeKey>,
    notes_by_project: &HashMap<Uuid, Vec<LocalNote>>,
) -> bool {
    let mut iter = set
        .iter()
        .filter_map(|k| classify_sibling_group(k, notes_by_project));
    let Some(first) = iter.next() else {
        return true;
    };
    iter.all(|g| g == first)
}

/// Shift+ArrowUp/Down extension: grow (or shrink) the multi-selection by
/// one row in `dir` direction (`+1` for down, `-1` for up). Anchor is the
/// last clicked row; the new selection is the inclusive range from anchor
/// to the row one step past `current` in the visible flat list. No-op if
/// `current` isn't in `visible_flat` or `dir` walks past the edge.
pub(super) fn extend_keyboard_selection(
    current: NodeKey,
    dir: i32,
    multi_selected: &mut dioxus::prelude::Signal<std::collections::BTreeSet<NodeKey>>,
    last_clicked: &dioxus::prelude::Signal<Option<NodeKey>>,
    visible_flat: &dioxus::prelude::Signal<Vec<NodeKey>>,
) {
    let flat = visible_flat.read().clone();
    let cur_pos = match flat.iter().position(|k| k == &current) {
        Some(p) => p,
        None => return,
    };
    let next_pos = if dir > 0 {
        cur_pos.checked_add(1).filter(|&i| i < flat.len())
    } else {
        cur_pos.checked_sub(1)
    };
    let next_idx = match next_pos {
        Some(p) => p,
        None => return,
    };
    let anchor = (*last_clicked.read()).unwrap_or(current);
    let anchor_pos = flat.iter().position(|k| k == &anchor).unwrap_or(cur_pos);
    let (lo, hi) = if anchor_pos <= next_idx {
        (anchor_pos, next_idx)
    } else {
        (next_idx, anchor_pos)
    };
    let mut set = multi_selected.read().clone();
    set.clear();
    for k in &flat[lo..=hi] {
        set.insert(*k);
    }
    multi_selected.set(set);
}

/// Track the most recently clicked row so Shift+click can compute a range
/// over the visible flattened tree.
#[derive(Clone, Copy)]
pub struct LastClicked(pub Signal<Option<NodeKey>>);

/// Source-of-truth for which row should currently hold DOM focus. Each
/// row's `use_effect` subscribes to this (plus `LocalNoteVersion` so the
/// effect also re-fires after data mutations) and calls `set_focus(true)`
/// on its captured `MountedData` when matched. Replacing imperative JS
/// `el.focus()` calls with this signal makes focus reactive — Dioxus
/// list-reorder diffs (Alt+↑/↓ moves) that drop browser focus self-heal
/// on the next render because the effect re-runs and re-focuses the row.
#[derive(Clone, Copy)]
pub struct FocusedNode(pub Signal<Option<NodeKey>>);

/// Plans-Phase-4-multiselect-aria: visible flattened tree across all
/// projects, in document order, respecting open/closed state. Updated by
/// ExplorerPanel whenever its inputs change; NoteRow / ProjectRow consume
/// it during Shift+click to compute proper ranges.
#[derive(Clone, Copy)]
pub struct VisibleFlat(pub Signal<Vec<NodeKey>>);

/// Plans-Phase-3-explorer-drag-drop-feedback: panel-scope mirror of the
/// per-project notes list. NoteRow's ondragstart reads this snapshot to
/// compute the descendant set of the dragged note in O(project size); the
/// result populates `DragDescendants` so subsequent `ondragover` events
/// can reject cycle-creating drops without retraversing.
#[derive(Clone, Copy)]
pub struct NotesByProjectCtx(pub Memo<HashMap<Uuid, Vec<LocalNote>>>);

/// Plans-Phase-8-explorer-undo: panel-scope handle to the explorer's
/// undo stack and the callback that pops + applies the latest inverse.
/// Rows read `history.read().is_empty()` (and `redo_is_empty()`) to gate
/// menu items and call `on_undo` / `on_redo` to fire them.
#[derive(Clone, Copy)]
pub struct ExplorerUndoCtx {
    pub history: Signal<history::ExplorerHistory>,
    pub on_undo: Callback<()>,
    pub on_redo: Callback<()>,
}

/// Bumped on every note mutation by component-scope writers (the
/// explorer's create / rename / delete handlers, the editor's save
/// scheduler, etc.). Detached-scope writers (`spawn_forever` tasks
/// — artifact cascade, workflow executor) instead bump
/// `crate::shell::companion_state::LOCAL_NOTE_VERSION`, which is a
/// `GlobalSignal` safe to write from any scope. A bridge effect in
/// `desktop.rs::Workspace` mirrors those global bumps back into this
/// `Signal`, so component readers see one unified version regardless
/// of which writer triggered the change.
#[derive(Clone, Copy)]
pub struct LocalNoteVersion(pub Signal<u64>);

/// App-scope: which projects are expanded in the workspace tree-state.
/// Promoted out of the explorer panel so the dedicated search panel can
/// expand the matching project on click without round-tripping through
/// `LocalProjectVersion`.
#[derive(Clone, Copy)]
pub struct WorkspaceOpenMap(pub Signal<HashMap<String, bool>>);

/// App-scope: shared `TreeStateQueue` instance. Both the explorer and the
/// search panel enqueue toggles through this queue so the debounced flush
/// coalesces writes from either panel.
#[derive(Clone, Copy)]
pub struct WorkspaceTreeQueueCtx(pub Signal<TreeStateQueue>);

/// App-scope reveal-request signal. External components (companion-chat
/// mention chips, note-link resolvers, etc.) write a note's UUID here
/// and the explorer's reveal effect picks it up: expands the owning
/// project, walks the parent chain expanding each ancestor, sets
/// `selected_note`, then clears the signal back to `None`. Decouples
/// "reveal this note" from the explorer's local `project_note_open`
/// signal which isn't accessible outside `ExplorerPanel`.
#[derive(Clone, Copy)]
pub struct RevealNoteRequest(pub Signal<Option<Uuid>>);

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

/// Plans-Phase-8-explorer-undo: rewrite `[[old]]` / `![[old]]` /
/// `[[project/old]]` / `![[project/old]]` references in every referrer
/// of `target_id` to point at `new_title` instead. Used by both the
/// rename callback (forward direction) and the undo-rename path
/// (inverse direction). Runs inside an existing spawn — no spawn here.
async fn rewrite_referrer_bodies(
    target_id: Uuid,
    old: &str,
    new_title: &str,
    project_name: &str,
    link_repo: Arc<dyn operon_store::repos::LocalNoteLinkRepository>,
    persistence: Arc<dyn Persistence>,
) {
    let referrers = link_repo.referrers_of(target_id).unwrap_or_default();
    for source in referrers {
        let source_str = source.to_string();
        let body_bytes = match persistence.load(&source_str).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(body) = String::from_utf8(body_bytes) else {
            continue;
        };
        let mut next = body.clone();
        next = next.replace(&format!("[[{old}]]"), &format!("[[{new_title}]]"));
        next = next.replace(&format!("![[{old}]]"), &format!("![[{new_title}]]"));
        let old_abs = format!("[[{project_name}/{old}]]");
        let new_abs = format!("[[{project_name}/{new_title}]]");
        next = next.replace(&old_abs, &new_abs);
        let old_abs_embed = format!("![[{project_name}/{old}]]");
        let new_abs_embed = format!("![[{project_name}/{new_title}]]");
        next = next.replace(&old_abs_embed, &new_abs_embed);
        if next != body {
            let _ = persistence.save(&source_str, next.as_bytes()).await;
            let _ = link_repo.rewrite_target_text(target_id, old, new_title);
            let _ = link_repo.rewrite_target_text(
                target_id,
                &format!("{project_name}/{old}"),
                &format!("{project_name}/{new_title}"),
            );
        }
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
    let crate::local_mode::ui::LocalBulkClipboard(bulk_clipboard) = use_context();
    let tabs: Signal<TabManager> = use_context();
    let save_scheduler: SaveScheduler = use_context();
    let _save_action: LocalSaveAction = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let WorkspaceOpenMap(workspace_open) = use_context();
    let WorkspaceTreeQueueCtx(tree_queue) = use_context();

    // Hotfix-v2: use_memo is read-driven and fires reliably under Dioxus
    // 0.7's render scope; use_effect was registering but never invoking
    // its closure, so mutations (rename, create, delete) bumped
    // `project_version` to no effect on the UI. Memos auto-recompute on
    // dependency change. Load failures surface as a visible banner via
    // `project_load_error` (rendered in the empty-state branch below).
    let project_load_error: Signal<Option<String>> = use_signal(|| None);
    let mut error_setter = project_load_error;
    let projects: Memo<Vec<LocalProject>> = {
        let repo = project_repo.clone();
        use_memo(move || {
            let v = *project_version.read();
            match repo.list() {
                Ok(rows) => {
                    eprintln!(
                        "operon::explorer: projects memo recomputed, version={v}, {} row(s)",
                        rows.len()
                    );
                    error_setter.set(None);
                    rows
                }
                Err(e) => {
                    let msg = format!("Could not load projects: {e}");
                    eprintln!("operon::explorer: {msg}");
                    error_setter.set(Some(msg));
                    Vec::new()
                }
            }
        })
    };

    // Workspace-scope tree-state snapshot (which projects are open) lives in
    // app scope (`WorkspaceOpenMap`) so the dedicated search panel shares it.
    // Hydration also lives in app scope; the explorer just reads.

    // Hotfix-v2: per-project note lists as a Memo so note_version bumps
    // (create/rename/delete) drive an automatic re-fetch. Same rationale
    // as `projects` above — use_effect wasn't firing in this scope.
    let notes_by_project: Memo<HashMap<Uuid, Vec<LocalNote>>> = {
        let repo = note_repo.clone();
        use_memo(move || {
            let v = *note_version.read();
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
            eprintln!(
                "operon::explorer: notes_by_project memo recomputed, version={v}, {} project(s)",
                map.len()
            );
            map
        })
    };
    // Plans-Phase-3-explorer-drag-drop-feedback: expose the same memo as
    // a context so descendant-aware DnD validation lives in NoteRow without
    // having to thread the snapshot through twenty props.
    use_context_provider(|| NotesByProjectCtx(notes_by_project));
    // Plans-Phase-4-explorer-undo-stack: panel-scope undo history. Capacity
    // 100; wrapped handlers below push inverses before each commit.
    let mut history: Signal<history::ExplorerHistory> =
        use_signal(history::ExplorerHistory::default);

    // Per-project note open/closed snapshots, lazily hydrated when a project opens.
    let project_note_open: Signal<HashMap<Uuid, HashMap<String, bool>>> = use_signal(HashMap::new);

    // Tree-state debounce queue is provided in app scope via
    // `WorkspaceTreeQueueCtx` so the search panel can enqueue toggles too.

    // ===== Phase-5: search state (title-only quick filter) =====
    let search_query: Signal<String> = use_signal(String::new);
    // Debounced query — only flushed 150ms after the user stops typing.
    let debounced_query: Signal<String> = use_signal(String::new);
    // Snapshot of the previously-selected note so Esc can restore focus.
    let prev_selection: Signal<Option<Uuid>> = use_signal(|| None);

    // VS Code-style "auto-reveal": when the active tab changes (e.g. user
    // clicks a tab in the strip), move the explorer's selected_note to that
    // tab's note so the gray-bg selection follows the user's editor focus,
    // not just the small blue left bar. peek() on selected_note avoids
    // subscribing the effect to its own writes — without that guard, the
    // .set() would re-fire the effect, and `prev_selection` would loop the
    // same way it did in image 9.
    {
        let mut selected_note_for_sync = selected_note;
        use_effect(move || {
            let active_id = tabs
                .read()
                .active()
                .and_then(|t| Uuid::parse_str(&t.note_id).ok());
            if let Some(id) = active_id {
                if *selected_note_for_sync.peek() != Some(id) {
                    selected_note_for_sync.set(Some(id));
                }
            }
        });
    }

    // Debounce: spawn a delay each time the query changes; only the last spawn
    // wins by checking a generation counter. Without cancellation, typing N
    // chars rapidly would queue N spawned tasks that all fire `set()` after
    // 150ms, causing N redundant panel re-renders on the trailing edge.
    let mut debounce_gen: Signal<u64> = use_signal(|| 0u64);
    {
        let mut debounced_setter = debounced_query;
        use_effect(move || {
            let q = search_query.read().clone();
            // Bump-and-capture the generation under which this spawn is born.
            // peek() avoids subscribing the effect to its own writes (read()
            // would create a feedback loop).
            let my_gen = {
                let next = *debounce_gen.peek() + 1;
                debounce_gen.set(next);
                next
            };
            spawn(async move {
                search::debounce_window().await;
                if *debounce_gen.peek() == my_gen {
                    debounced_setter.set(q);
                }
            });
        });
    }

    // Cross-note content search lives in `crate::plugins::local_search`; the
    // explorer's input is a title-only quick filter, so there is no body
    // cache to load here.

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
    let LastClicked(mut last_clicked_for_clear) = use_context();
    let FocusedNode(mut focused_node_for_clear) = use_context();

    // ===== Project handlers =====
    let on_select_project = use_callback(move |id: Uuid| {
        selected_project.set(Some(id));
        selected_note.set(None);
    });

    let project_repo_for_create = project_repo.clone();
    let mut error_setter_for_create = project_load_error;
    let on_add_project = move |_| {
        eprintln!("operon::explorer: on_add_project fired");
        match project_repo_for_create.create("") {
            Ok(p) => {
                eprintln!("operon::explorer: created project {} ({})", p.id, p.name);
                project_version.with_mut(|v| *v += 1);
                selected_project.set(Some(p.id));
                renaming_project_setter.set(Some(p.id));
                error_setter_for_create.set(None);
            }
            Err(e) => {
                let msg = format!("Could not create project: {e}");
                eprintln!("operon::explorer: {msg}");
                error_setter_for_create.set(Some(msg));
            }
        }
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

    // M1-companion-claude-code: bind / clear the project's git repository
    // path. Persists to SQL and bumps `project_version` so the explorer +
    // companion-pane subscribers refresh.
    let project_repo_for_set_repo = project_repo.clone();
    let on_set_repo_path = use_callback(
        move |(id, new_path): (Uuid, Option<std::path::PathBuf>)| {
            match project_repo_for_set_repo.set_repo_path(id, new_path.as_deref()) {
                Ok(()) => {
                    project_version.with_mut(|v| *v += 1);
                }
                Err(e) => eprintln!("operon: set repo_path failed: {e}"),
            }
        },
    );

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

    // Auto-open the project at the workspace level so a freshly-created
    // root/child/sibling/image note becomes visible even when the project
    // chevron was collapsed at the time of creation. Mirrors the open-side
    // branch of `on_toggle_project`: flips `workspace_open[project_id]`,
    // enqueues the persistence write, and lazy-hydrates the per-project
    // note-open map on first open.
    let queue_for_open_project = tree_queue;
    let mut workspace_open_for_open_project = workspace_open;
    let mut project_note_open_for_open_project = project_note_open;
    let tree_repo_for_open_project = tree_repo.clone();
    let open_project_workspace = move |project_id: Uuid| {
        let key = project_id.to_string();
        let already_open = workspace_open_for_open_project
            .read()
            .get(&key)
            .copied()
            .unwrap_or(false);
        if already_open {
            return;
        }
        workspace_open_for_open_project.with_mut(|m| {
            m.insert(key.clone(), true);
        });
        queue_for_open_project
            .read()
            .enqueue(SCOPE_WORKSPACE, key, true);
        if !project_note_open_for_open_project
            .read()
            .contains_key(&project_id)
        {
            match tree_repo_for_open_project.snapshot_for_scope(&scope_for_project(project_id)) {
                Ok(snap) => project_note_open_for_open_project.with_mut(|m| {
                    m.insert(project_id, snap);
                }),
                Err(e) => eprintln!(
                    "operon: snapshot_for_scope project:{project_id} failed (open-on-create): {e}"
                ),
            }
        }
    };

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
        let Some(list) = snap.get(&project_id) else {
            return;
        };
        // Build a parent lookup once so the ancestor walk is O(depth)
        // instead of O(depth * project size).
        let parent_by_id: HashMap<Uuid, Option<Uuid>> =
            list.iter().map(|n| (n.id, n.parent_id)).collect();
        while let Some(id) = cursor {
            let key = id.to_string();
            project_note_open_for_expand.with_mut(|map| {
                map.entry(project_id).or_default().insert(key.clone(), true);
            });
            queue_for_expand.read().enqueue(scope.clone(), key, true);
            cursor = parent_by_id.get(&id).copied().flatten();
        }
    };

    // Cross-component reveal: external callers (companion-chat mention
    // chips, the markdown note-link resolver, etc.) write a target note
    // id to `RevealNoteRequest`; we walk it here. Expanding the owning
    // project + the parent chain makes the note visible regardless of
    // current tree-state, then `selected_note` puts the cursor on it.
    // Cleared back to None at the end so the next bump re-fires this
    // effect even for the same note.
    {
        let RevealNoteRequest(mut reveal_request) = use_context();
        let notes_by_project_for_reveal = notes_by_project;
        let mut open_project_for_reveal = open_project_workspace.clone();
        let mut expand_ancestors_for_reveal = expand_ancestors.clone();
        let mut selected_note_for_reveal = selected_note;
        use_effect(move || {
            let Some(target) = *reveal_request.read() else {
                return;
            };
            // Locate the note's project + parent chain. The memo is
            // keyed on `note_version`, so newly-created notes show up
            // by the time the user can plausibly click a mention to
            // them. If the lookup misses (cross-vault id, deleted
            // note), still clear the signal so we don't loop.
            let project_and_parent = {
                let snap = notes_by_project_for_reveal.read();
                snap.iter().find_map(|(pid, list)| {
                    list.iter()
                        .find(|n| n.id == target)
                        .map(|n| (*pid, n.parent_id))
                })
            };
            if let Some((pid, parent_id)) = project_and_parent {
                open_project_for_reveal(pid);
                expand_ancestors_for_reveal(pid, parent_id);
                selected_note_for_reveal.set(Some(target));
            }
            reveal_request.set(None);
        });
    }

    let note_repo_for_add_root = note_repo.clone();
    let mut open_project_for_root = open_project_workspace.clone();
    let on_add_root_markdown_note = use_callback(move |project_id: Uuid| {
        open_project_for_root(project_id);
        match note_repo_for_add_root.create(project_id, None, "") {
            Ok(n) => {
                // Plans-Phase-10: undoable create.
                history.write().push(history::ExplorerAction::Create {
                    id: n.id,
                    blob_path: None,
                });
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(Some(n.id));
            }
            Err(e) => eprintln!("operon: create local_note failed: {e}"),
        }
    });

    // Operon-Phase-3-note-kinds: Image-note creation is now placeholder-
    // first. The historical OS file picker has moved into the empty-state
    // pane of the image editor (`local_mode::editor::try_render_image_view`
    // empty branch) — clicking "Image" in any + dropdown or right-click
    // submenu mints an empty Image note (blob_path = None), opens its tab,
    // and lets the user pick / paste / drop a file from inside the editor.
    // No async / file system work happens here, so this is a lean,
    // synchronous callback.
    let note_repo_for_add_image = note_repo.clone();
    let crate::local_mode::CurrentVaultRoot(vault_root_signal) = use_context::<crate::local_mode::CurrentVaultRoot>();
    let mut open_project_for_image = open_project_workspace.clone();
    let mut expand_ancestors_for_image = expand_ancestors.clone();
    let on_pick_image_note = use_callback(move |
        (project_id, parent_id, sibling_after_idx): (Uuid, Option<Uuid>, Option<i64>),
    | {
        open_project_for_image(project_id);
        expand_ancestors_for_image(project_id, parent_id);
        match note_repo_for_add_image.create_with_kind(project_id, parent_id, "", NoteKind::Image) {
            Ok(row) => {
                if let Some(idx) = sibling_after_idx {
                    if let Err(e) = note_repo_for_add_image.move_to(row.id, project_id, parent_id, idx) {
                        eprintln!("operon: image sibling move_to failed: {e}");
                    }
                }
                history.write().push(history::ExplorerAction::Create {
                    id: row.id,
                    blob_path: None,
                });
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(Some(row.id));
            }
            Err(e) => eprintln!("operon: create image placeholder failed: {e}"),
        }
    });

    // Operon-Phase-1-note-kinds: unified project-level Add note dispatch.
    // Markdown still takes the auto-rename fast path. Image continues
    // through the picker callback. Every other kind (Mdx, Code, Kanban,
    // Canvas, Excalidraw) is created at the root via create_with_kind —
    // empty content, inline rename triggered, format dispatched at render
    // time via NoteKind::format_id().
    let note_repo_for_add_project = note_repo.clone();
    let mut open_project_for_add = open_project_workspace.clone();
    let persistence_for_add_project: Arc<dyn Persistence> = use_context();
    let on_add_project_note =
        use_callback(move |(project_id, kind): (Uuid, CreatableKind)| match kind {
            CreatableKind::Plain(NoteKind::Markdown) => {
                on_add_root_markdown_note.call(project_id);
            }
            CreatableKind::Plain(NoteKind::Image) => {
                on_pick_image_note.call((project_id, None, None));
            }
            CreatableKind::Plain(other) => {
                open_project_for_add(project_id);
                match note_repo_for_add_project.create_with_kind(project_id, None, "", other) {
                    Ok(n) => {
                        history.write().push(history::ExplorerAction::Create {
                            id: n.id,
                            blob_path: None,
                        });
                        note_version.with_mut(|v| *v += 1);
                        renaming_note_setter.set(Some(n.id));
                    }
                    Err(e) => eprintln!("operon: create root {} note failed: {e}", other.as_str()),
                }
            }
            CreatableKind::Artifact(akind) => {
                open_project_for_add(project_id);
                match note_repo_for_add_project.create_with_kind(
                    project_id,
                    None,
                    "",
                    NoteKind::Artifact,
                ) {
                    Ok(n) => {
                        history.write().push(history::ExplorerAction::Create {
                            id: n.id,
                            blob_path: None,
                        });
                        let body = creatable_kind::scaffold_body(&akind);
                        let persistence = persistence_for_add_project.clone();
                        let new_id = n.id;
                        spawn(async move {
                            if let Err(e) = persistence
                                .save(&new_id.to_string(), body.as_bytes())
                                .await
                            {
                                eprintln!(
                                    "operon: write scaffold body for root artifact {new_id}: {e}"
                                );
                            }
                        });
                        note_version.with_mut(|v| *v += 1);
                        renaming_note_setter.set(Some(n.id));
                    }
                    Err(e) => eprintln!(
                        "operon: create root artifact ({}) failed: {e}",
                        akind.as_str()
                    ),
                }
            }
        });

    // Three-tier SDLC: dedicated "+ New phase" project action. Creates
    // a `NoteKind::Phase` note at project root with an empty phase
    // frontmatter (`phase_label: ""`), opens the project, and
    // triggers inline rename so the user types the phase name
    // (Discovery, Multiplayer MVP, …). Phase ordering falls back to
    // `created_at_ms` until the user adds an explicit `phase_order`
    // field — sufficient for the common case where phases are
    // authored in chronological order.
    let note_repo_for_add_phase = note_repo.clone();
    let persistence_for_add_phase: Arc<dyn Persistence> = use_context();
    let mut open_project_for_phase = open_project_workspace.clone();
    let on_add_project_phase = use_callback(move |project_id: Uuid| {
        open_project_for_phase(project_id);
        match note_repo_for_add_phase.create_with_kind(
            project_id,
            None,
            "",
            NoteKind::Phase,
        ) {
            Ok(n) => {
                history.write().push(history::ExplorerAction::Create {
                    id: n.id,
                    blob_path: None,
                });
                // Seed an empty phase frontmatter so downstream
                // tooling (cascade re-instancing, phase listing) can
                // parse the note before the user adds any content.
                let body = crate::plugins::phase::serialize(
                    &crate::plugins::phase::PhaseFrontmatter::default(),
                    "",
                );
                let persistence = persistence_for_add_phase.clone();
                let new_id = n.id;
                spawn(async move {
                    if let Err(e) = persistence
                        .save(&new_id.to_string(), body.as_bytes())
                        .await
                    {
                        eprintln!(
                            "operon: write phase scaffold body for {new_id}: {e}"
                        );
                    }
                });
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(Some(n.id));
            }
            Err(e) => eprintln!("operon: create phase failed: {e}"),
        }
    });

    // ===== Note handlers =====
    let mut tabs_for_select = tabs;
    let scheduler_for_select = save_scheduler.clone();
    // Plans-Phase-2-editor-auto-focus: the editor host listens to this
    // app-scope signal and grants focus when its note id matches. Gated on
    // `renaming_note.is_none()` so the rename input keeps the caret.
    let crate::editor::RequestEditorFocus(mut request_editor_focus) = use_context();
    // Plans-Phase-9-monaco-desktop (rev 15): clone the persistence
    // handle once for the on_select_note closure (and any future
    // async loaders) so the spawned future doesn't capture the
    // outer context handle.
    let persistence_for_select = persistence.clone();
    let on_select_note = use_callback(move |note_id: Uuid| {
        selected_note.set(Some(note_id));
        selected_project.set(None);
        // Find note metadata to get the title + kind; fall back to id +
        // Markdown so the editor still mounts (textarea path) for missing
        // rows.
        let (title, kind) = notes_by_project
            .read()
            .values()
            .flat_map(|list| list.iter())
            .find(|n| n.id == note_id)
            .map(|n| (n.title.clone(), n.kind))
            .unwrap_or_else(|| (note_id.to_string(), NoteKind::Markdown));

        // Plans-Phase-9-monaco-desktop (rev 15): "click on note in
        // explorer" buffer-init logic.
        // 1. If an Edit-mode tab already exists, focus it (no new
        //    buffer — keep what the user typed).
        // 2. Otherwise, if any tab (View / Split / LivePreview) for
        //    this note exists, copy its in-memory buffer into the
        //    new Edit tab so the user keeps working from the same
        //    text they just had open as a preview.
        // 3. If no tab exists at all, spawn an async load from the
        //    Persistence trait (SQLite for desktop, OPFS for web)
        //    and open the tab with the saved content. Falls back to
        //    empty if the note doesn't exist on disk yet.
        let note_id_str = note_id.to_string();
        let existing_edit = {
            let snap = tabs_for_select.read();
            let id = snap
                .iter()
                .find(|t| t.note_id == note_id_str
                    && matches!(t.mode, crate::editor::EditorMode::Edit))
                .map(|t| t.id);
            id
        };
        if let Some(tid) = existing_edit {
            tabs_for_select.write().activate(tid);
        } else {
            let inherited = {
                let snap = tabs_for_select.read();
                let c = snap
                    .iter()
                    .find(|t| t.note_id == note_id_str)
                    .map(|t| t.content.clone());
                c
            };
            // Plans-Phase-9-monaco-desktop (rev 20): drive the persistence
            // load synchronously on desktop and pass the bytes as the
            // tab's initial content, so Monaco mounts with the saved body
            // already populated via `monaco.editor.create({value: …})`.
            // Earlier revs spawned an async load and pushed `setContent`
            // post-mount; in practice the first post-mount `eval.send`
            // is dropped between Wry's `evaluate_script` and the JS
            // `Channel.recv()` even though `eval.send` returns Ok and
            // the bootstrap recv loop is alive. `FilesystemPersistence`
            // wraps a synchronous `std::fs::read`, so `block_on` resolves
            // in one poll — no UI block. On wasm, `OpfsPersistence` is
            // genuinely async and `block_on` would deadlock the browser
            // thread, so the spawn-then-set_content path stays there.
            #[cfg(not(target_arch = "wasm32"))]
            let synchronous_content = match inherited {
                Some(content) => content,
                None => {
                    let pers = persistence_for_select.clone();
                    let note_id_load = note_id_str.clone();
                    match futures::executor::block_on(pers.load(&note_id_load)) {
                        Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
                        Err(PersistError::NotFound) => String::new(),
                        Err(e) => {
                            eprintln!(
                                "operon: load error note_id={note_id_load}: {e:?}"
                            );
                            String::new()
                        }
                    }
                }
            };
            #[cfg(target_arch = "wasm32")]
            let synchronous_content = inherited.as_ref().cloned().unwrap_or_default();

            let new_tab_id = open_local_note_tab(
                tabs_for_select,
                scheduler_for_select.clone(),
                note_id,
                title.clone(),
                synchronous_content,
                kind,
            );
            // Web build: keep the existing spawn-then-set_content path
            // since OpfsPersistence's load is genuinely async. The
            // `monaco_ready` gate in MonacoEditorHost handles the
            // post-mount setContent timing on this path.
            #[cfg(target_arch = "wasm32")]
            {
                if inherited.is_none() {
                    let pers = persistence_for_select.clone();
                    let mut tabs_handle = tabs_for_select;
                    let note_id_load = note_id_str.clone();
                    spawn(async move {
                        match pers.load(&note_id_load).await {
                            Ok(bytes) => {
                                if let Ok(content) = String::from_utf8(bytes) {
                                    tabs_handle.write().set_content(new_tab_id, content);
                                }
                            }
                            Err(PersistError::NotFound) => {}
                            Err(e) => eprintln!(
                                "operon: load error note_id={note_id_load}: {e:?}"
                            ),
                        }
                    });
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = new_tab_id;
        }
        let _ = tabs_for_select.write();
        // Plans-Phase-2: grant focus only when no rename input is alive.
        if renaming_note.read().is_none() {
            request_editor_focus.set(Some(note_id.to_string()));
        }
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

        // Plans-Phase-4 / Plans-Phase-8: capture prev title + project name
        // BEFORE the repo write so undo can run rewrite_referrer_bodies in
        // the reverse direction. Push only after success.
        let prev_title_for_undo = old_title.clone();
        let project_name_for_undo = project_name.clone();
        match note_repo_for_rename.rename(id, &new_title) {
            Ok(()) => {
                if let Some(prev) = prev_title_for_undo {
                    // Plans-Phase-10: when prev_title was empty, this rename
                    // is the user committing the title of a freshly-created
                    // note. Skip pushing a Rename inverse — the Create
                    // entry the create handler already pushed will undo
                    // both the title and the row in one shot. Keeps the
                    // stack tidy (one entry per user gesture).
                    if !prev.is_empty() && prev != new_title {
                        history.write().push(history::ExplorerAction::Rename {
                            id,
                            prev_title: prev,
                            project_name: project_name_for_undo,
                            new_title: new_title.clone(),
                        });
                    }
                }
                note_version.with_mut(|v| *v += 1);
                renaming_note_setter.set(None);

                // Walk every referrer and rewrite `[[OldTitle]]` / etc to
                // the new equivalents. Async via `spawn` so the rename
                // callback returns immediately. Plans-Phase-8 factored the
                // body of this loop into `rewrite_referrer_bodies` so the
                // undo path can run it in reverse.
                if let (Some(old), Some(proj_name)) = (old_title, project_name.clone()) {
                    if old != new_title {
                        let link_repo = link_repo_for_rename.clone();
                        let persistence = persistence_for_rename.clone();
                        let new_title_owned = new_title.clone();
                        spawn(async move {
                            rewrite_referrer_bodies(
                                id,
                                &old,
                                &new_title_owned,
                                &proj_name,
                                link_repo,
                                persistence,
                            )
                            .await;
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

    let note_repo_for_add_child = note_repo.clone();
    let mut expand_ancestors_for_child = expand_ancestors.clone();
    let mut open_project_for_child = open_project_workspace.clone();
    let persistence_for_add_child: Arc<dyn Persistence> = use_context();
    // Kind-aware add-child dispatch. `Plain(Image)` opens the picker;
    // other plain kinds take the existing create_with_kind fast path;
    // `Artifact(kind)` creates a NoteKind::Artifact + writes the
    // matching scaffold body via Persistence so the note slots into
    // the cascade pipeline immediately.
    let on_add_child_note = use_callback(move |(parent_id, kind): (Uuid, CreatableKind)| {
        let project_id = notes_by_project
            .read()
            .iter()
            .find_map(|(pid, list)| list.iter().find(|n| n.id == parent_id).map(|_| *pid));
        let Some(project_id) = project_id else {
            eprintln!("operon: add child note: parent {parent_id} not found");
            return;
        };
        open_project_for_child(project_id);
        expand_ancestors_for_child(project_id, Some(parent_id));
        match kind {
            CreatableKind::Plain(NoteKind::Image) => {
                on_pick_image_note.call((project_id, Some(parent_id), None));
            }
            CreatableKind::Plain(other) => {
                match note_repo_for_add_child.create_with_kind(project_id, Some(parent_id), "", other) {
                    Ok(n) => {
                        history.write().push(history::ExplorerAction::Create {
                            id: n.id,
                            blob_path: None,
                        });
                        note_version.with_mut(|v| *v += 1);
                        renaming_note_setter.set(Some(n.id));
                    }
                    Err(e) => eprintln!("operon: create child {} note failed: {e}", other.as_str()),
                }
            }
            CreatableKind::Artifact(akind) => {
                match note_repo_for_add_child.create_with_kind(
                    project_id,
                    Some(parent_id),
                    "",
                    NoteKind::Artifact,
                ) {
                    Ok(n) => {
                        history.write().push(history::ExplorerAction::Create {
                            id: n.id,
                            blob_path: None,
                        });
                        // Seed the body with frontmatter + section
                        // headers before the user opens the note,
                        // so the cascade engine recognises its
                        // `artifact_kind` on the next pass and the
                        // user sees the right scaffold on click.
                        let body = creatable_kind::scaffold_body(&akind);
                        let persistence = persistence_for_add_child.clone();
                        let new_id = n.id;
                        spawn(async move {
                            if let Err(e) = persistence
                                .save(&new_id.to_string(), body.as_bytes())
                                .await
                            {
                                eprintln!(
                                    "operon: write scaffold body for artifact {new_id}: {e}"
                                );
                            }
                        });
                        note_version.with_mut(|v| *v += 1);
                        renaming_note_setter.set(Some(n.id));
                    }
                    Err(e) => eprintln!(
                        "operon: create child artifact ({}) failed: {e}",
                        akind.as_str()
                    ),
                }
            }
        }
    });

    // Plans-Phase-3-note-id-create: insert a new sibling note immediately
    // after the target. Creates with the same `parent_id` as the target,
    // then `move_to` to land at `target.sibling_index + 1` (`move_to`
    // shifts the dense ordering). Triggers inline rename on the new row
    // and expands ancestors so the new row is visible.
    let note_repo_for_add_sibling = note_repo.clone();
    let mut expand_ancestors_for_sibling = expand_ancestors.clone();
    let mut open_project_for_sibling = open_project_workspace.clone();
    let persistence_for_add_sibling: Arc<dyn Persistence> = use_context();
    // Kind-aware add-sibling. Mirrors `on_add_child_note`: Plain(Image)
    // routes through the picker; other Plain kinds take the
    // create-then-move_to path; `Artifact(kind)` additionally writes
    // the scaffold body via Persistence.
    let on_add_sibling_note = use_callback(move |(target_id, kind): (Uuid, CreatableKind)| {
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
        open_project_for_sibling(project_id);
        expand_ancestors_for_sibling(project_id, parent_id);
        match kind {
            CreatableKind::Plain(NoteKind::Image) => {
                on_pick_image_note.call((project_id, parent_id, Some(target_idx + 1)));
            }
            CreatableKind::Plain(other) => {
                match note_repo_for_add_sibling.create_with_kind(project_id, parent_id, "", other) {
                    Ok(n) => {
                        if let Err(e) = note_repo_for_add_sibling.move_to(
                            n.id,
                            project_id,
                            parent_id,
                            target_idx + 1,
                        ) {
                            eprintln!("operon: add sibling: move_to failed: {e}");
                        }
                        history.write().push(history::ExplorerAction::Create {
                            id: n.id,
                            blob_path: None,
                        });
                        note_version.with_mut(|v| *v += 1);
                        renaming_note_setter.set(Some(n.id));
                    }
                    Err(e) => eprintln!("operon: create sibling {} note failed: {e}", other.as_str()),
                }
            }
            CreatableKind::Artifact(akind) => {
                match note_repo_for_add_sibling.create_with_kind(
                    project_id,
                    parent_id,
                    "",
                    NoteKind::Artifact,
                ) {
                    Ok(n) => {
                        if let Err(e) = note_repo_for_add_sibling.move_to(
                            n.id,
                            project_id,
                            parent_id,
                            target_idx + 1,
                        ) {
                            eprintln!("operon: add sibling: move_to failed: {e}");
                        }
                        history.write().push(history::ExplorerAction::Create {
                            id: n.id,
                            blob_path: None,
                        });
                        let body = creatable_kind::scaffold_body(&akind);
                        let persistence = persistence_for_add_sibling.clone();
                        let new_id = n.id;
                        spawn(async move {
                            if let Err(e) = persistence
                                .save(&new_id.to_string(), body.as_bytes())
                                .await
                            {
                                eprintln!(
                                    "operon: write scaffold body for artifact {new_id}: {e}"
                                );
                            }
                        });
                        note_version.with_mut(|v| *v += 1);
                        renaming_note_setter.set(Some(n.id));
                    }
                    Err(e) => eprintln!(
                        "operon: create sibling artifact ({}) failed: {e}",
                        akind.as_str()
                    ),
                }
            }
        }
    });

    // ===== Phase-4 handlers: indent/outdent/move/clipboard =====
    // Plans-Phase-4-explorer-undo-stack: capture (project, parent, sibling)
    // before each structural move so undo can restore the prior position.
    // Returns Some when a snapshot was found; None for a missing row (we
    // log + skip the push instead of mutating).
    let snapshot_position = move |id: Uuid| -> Option<(Uuid, Option<Uuid>, i64)> {
        let snap = notes_by_project.read();
        for (pid, list) in snap.iter() {
            if let Some(n) = list.iter().find(|n| n.id == id) {
                return Some((*pid, n.parent_id, n.sibling_index));
            }
        }
        None
    };
    let note_repo_for_indent = note_repo.clone();
    let on_indent_note = use_callback(move |id: Uuid| {
        let prev = snapshot_position(id);
        match note_repo_for_indent.indent(id) {
            Ok(()) => {
                if let Some((project_id, prev_parent, prev_index)) = prev {
                    history.write().push(history::ExplorerAction::MoveWithin {
                        id,
                        project_id,
                        prev_parent,
                        prev_index,
                    });
                }
                note_version.with_mut(|v| *v += 1);
            }
            Err(e) => eprintln!("operon: indent note failed: {e}"),
        }
    });
    let note_repo_for_outdent = note_repo.clone();
    let on_outdent_note = use_callback(move |id: Uuid| {
        let prev = snapshot_position(id);
        match note_repo_for_outdent.outdent(id) {
            Ok(()) => {
                if let Some((project_id, prev_parent, prev_index)) = prev {
                    history.write().push(history::ExplorerAction::MoveWithin {
                        id,
                        project_id,
                        prev_parent,
                        prev_index,
                    });
                }
                note_version.with_mut(|v| *v += 1);
            }
            Err(e) => eprintln!("operon: outdent note failed: {e}"),
        }
    });
    let note_repo_for_up = note_repo.clone();
    let on_move_up_note = use_callback(move |id: Uuid| {
        let prev = snapshot_position(id);
        match note_repo_for_up.move_up(id) {
            Ok(()) => {
                if let Some((project_id, prev_parent, prev_index)) = prev {
                    history.write().push(history::ExplorerAction::MoveWithin {
                        id,
                        project_id,
                        prev_parent,
                        prev_index,
                    });
                }
                note_version.with_mut(|v| *v += 1);
            }
            Err(e) => eprintln!("operon: move_up note failed: {e}"),
        }
    });
    let note_repo_for_down = note_repo.clone();
    let on_move_down_note = use_callback(move |id: Uuid| {
        let prev = snapshot_position(id);
        match note_repo_for_down.move_down(id) {
            Ok(()) => {
                if let Some((project_id, prev_parent, prev_index)) = prev {
                    history.write().push(history::ExplorerAction::MoveWithin {
                        id,
                        project_id,
                        prev_parent,
                        prev_index,
                    });
                }
                note_version.with_mut(|v| *v += 1);
            }
            Err(e) => eprintln!("operon: move_down note failed: {e}"),
        }
    });

    // Plans-Phase-4-explorer-undo-stack: pop the latest inverse and apply.
    // Failures log + emit a toast; the entry is still consumed so the
    // user moves on.
    let note_repo_for_undo = note_repo.clone();
    // Plans-Phase-10: cloned for the Create-undo arm's blob-GC walk.
    let project_repo_for_undo = project_repo.clone();
    let crate::local_mode::desktop::LocalNoteLinkRepo(link_repo_for_undo) =
        use_context::<crate::local_mode::desktop::LocalNoteLinkRepo>();
    let persistence_for_undo: Arc<dyn Persistence> = use_context();
    // Plans-Phase-11: clones for the on_redo closure (which also touches
    // the link repo + persistence to apply the forward-direction
    // referrer-body rewrite during a Rename redo).
    let link_repo_for_redo = link_repo_for_undo.clone();
    let persistence_for_redo = persistence_for_undo.clone();
    // Plans-Phase-8: app-scope toast slot for surfacing undo failures.
    let crate::local_mode::ui::ToastSlot(mut toast_slot) = use_context();
    let on_undo = use_callback(move |_: ()| {
        let action = history.write().pop();
        let Some(action) = action else { return };
        // Plans-Phase-11: clone the action up front so we can push it
        // onto the redo deque after a successful apply, *if* the variant
        // supports redo. Variants that don't (MoveWithin / Paste /
        // Create) are simply dropped after applying.
        let redo_clone = action.clone();
        match action {
            history::ExplorerAction::Rename {
                id,
                prev_title,
                project_name,
                new_title,
            } => {
                // Restore the title first; same call shape as the forward path.
                if let Err(e) = note_repo_for_undo.rename(id, &prev_title) {
                    eprintln!("operon: undo rename failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Undo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
                // Plans-Phase-8: rewrite referrer bodies back —
                // `new_title` was the substring the forward rename
                // injected; replacing it with `prev_title` is the
                // inverse. Async; same fire-and-forget semantics as
                // the forward rewrite.
                if let Some(proj_name) = project_name {
                    if prev_title != new_title {
                        let link_repo = link_repo_for_undo.clone();
                        let persistence = persistence_for_undo.clone();
                        let prev_title_owned = prev_title.clone();
                        let new_title_owned = new_title.clone();
                        spawn(async move {
                            rewrite_referrer_bodies(
                                id,
                                &new_title_owned,
                                &prev_title_owned,
                                &proj_name,
                                link_repo,
                                persistence,
                            )
                            .await;
                        });
                    }
                }
                // Plans-Phase-11: Rename is fully reversible (both titles
                // stored in the variant), so push to redo.
                history.write().push_redo(redo_clone);
            }
            history::ExplorerAction::MoveWithin {
                id,
                project_id,
                prev_parent,
                prev_index,
            } => {
                if let Err(e) =
                    note_repo_for_undo.move_to(id, project_id, prev_parent, prev_index)
                {
                    eprintln!("operon: undo move failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Undo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
                // Plans-Phase-11: MoveWithin not redoable — see the
                // variant doc-comment for rationale.
            }
            history::ExplorerAction::Delete { snapshot } => {
                if let Err(e) = note_repo_for_undo.restore_subtree(&snapshot) {
                    eprintln!("operon: undo delete failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Undo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
                // Plans-Phase-11: Delete is redoable — the snapshot
                // carries everything needed for the forward direction
                // (re-delete root_id).
                history.write().push_redo(redo_clone);
            }
            history::ExplorerAction::Paste { pasted_root_id } => {
                // Cascade kills descendants automatically via the FK.
                if let Err(e) = note_repo_for_undo.delete(pasted_root_id) {
                    eprintln!("operon: undo paste failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Undo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
                // Plans-Phase-11: Paste forward direction would need the
                // original clipboard payload, which we no longer have.
                // Skip from redo.
            }
            history::ExplorerAction::Create { id, blob_path } => {
                // Plans-Phase-10: undo of create deletes the row (cascade
                // kills any children the user has since added beneath it,
                // but for a freshly-created note there usually aren't any)
                // and removes the on-disk blob for image notes — only if
                // no other note still references it.
                if let Err(e) = note_repo_for_undo.delete(id) {
                    eprintln!("operon: undo create failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Undo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
                if let Some(rel) = blob_path {
                    if let Some(vault) = vault_root_signal.read().clone() {
                        let projects = project_repo_for_undo.list().unwrap_or_default();
                        let still_referenced = projects.iter().any(|p| {
                            note_repo_for_undo
                                .list_for_project(p.id)
                                .map(|notes| {
                                    notes.iter().any(|n| n.blob_path.as_deref() == Some(rel.as_str()))
                                })
                                .unwrap_or(false)
                        });
                        if !still_referenced {
                            let _ = std::fs::remove_file(vault.path().join(&rel));
                        }
                    }
                }
            }
        }
        note_version.with_mut(|v| *v += 1);
    });

    // Plans-Phase-11-redo-stack: pop from the redo deque and apply the
    // forward direction. Mirrors `on_undo` — same toast-on-failure
    // pattern, same note_version bump on success. Only the variants
    // that were re-pushed onto redo by `on_undo` (Rename, Delete) end
    // up here; MoveWithin / Paste / Create are dropped during undo so
    // pop_redo never sees them.
    let note_repo_for_redo = note_repo.clone();
    let on_redo = use_callback(move |_: ()| {
        let action = history.write().pop_redo();
        let Some(action) = action else { return };
        let undo_clone = action.clone();
        match action {
            history::ExplorerAction::Rename {
                id,
                prev_title,
                project_name,
                new_title,
            } => {
                // Forward direction: rename (id, new_title) + rewrite in
                // the forward direction (prev_title -> new_title).
                if let Err(e) = note_repo_for_redo.rename(id, &new_title) {
                    eprintln!("operon: redo rename failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Redo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
                if let Some(proj_name) = project_name {
                    if prev_title != new_title {
                        let link_repo = link_repo_for_redo.clone();
                        let persistence = persistence_for_redo.clone();
                        let prev_title_owned = prev_title.clone();
                        let new_title_owned = new_title.clone();
                        spawn(async move {
                            rewrite_referrer_bodies(
                                id,
                                &prev_title_owned,
                                &new_title_owned,
                                &proj_name,
                                link_repo,
                                persistence,
                            )
                            .await;
                        });
                    }
                }
            }
            history::ExplorerAction::Delete { ref snapshot } => {
                // Forward direction: re-delete the root id. Cascade kills
                // descendants automatically via the FK.
                if let Err(e) = note_repo_for_redo.delete(snapshot.root_id) {
                    eprintln!("operon: redo delete failed: {e}");
                    toast_slot.set(Some(crate::local_mode::ui::Toast {
                        message: format!("Redo failed: {e}"),
                        kind: crate::local_mode::ui::ToastKind::Error,
                    }));
                    return;
                }
            }
            // Variants dropped during undo never reach pop_redo, but the
            // exhaustive match keeps the compiler honest if a future
            // variant lands.
            history::ExplorerAction::MoveWithin { .. }
            | history::ExplorerAction::Paste { .. }
            | history::ExplorerAction::Create { .. } => return,
        }
        history.write().push_undo(undo_clone);
        note_version.with_mut(|v| *v += 1);
    });

    // Plans-Phase-8-explorer-undo: provide the history + on_undo as a
    // single context so NoteRow / ProjectRow can render an "Undo last
    // action" menu item without prop-drilling.
    use_context_provider(|| ExplorerUndoCtx {
        history,
        on_undo,
        on_redo,
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
    // Plans-Phase-4-multiselect-aria: bulk-delete now also handles
    // Project members of the multi-set (per the user's "notes + projects"
    // scope decision). We delete projects after notes so any project that
    // was just stripped of children still vanishes.
    let project_repo_for_bulk_project_delete = project_repo.clone();
    let crate::local_mode::CurrentVaultRoot(vault_root_for_bulk_gc) = use_context();
    let mut tabs_for_bulk_delete = tabs;
    let on_confirm_bulk_delete = use_callback(move |_: ()| {
        let snapshot = multi_selected.read().clone();
        // Plans-Phase-6-image-notes: snapshot blob_paths to potentially
        // GC. We collect every blob_path of the targets + any descendants
        // before the delete tx fires (FK cascade loses them after). Same
        // walk also collects the note ids whose tabs we must close after
        // the deletes commit.
        let mut blobs: Vec<String> = Vec::new();
        let mut deleted_note_ids: Vec<String> = Vec::new();
        let snap = notes_by_project.read();
        let target_ids: std::collections::HashSet<Uuid> = snapshot
            .iter()
            .filter_map(|k| match k {
                NodeKey::Note(id) => Some(*id),
                NodeKey::Project(_) => None,
            })
            .collect();
        for tid in &target_ids {
            deleted_note_ids.push(tid.to_string());
        }
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
                    if !target_ids.contains(&n.id) {
                        deleted_note_ids.push(n.id.to_string());
                    }
                }
            }
        }
        drop(snap);

        // Plans-Phase-8-explorer-undo: capture each top-level deleted node
        // as its own Delete inverse. Bulk-undo therefore restores the
        // selection one entry at a time (LIFO via repeated Cmd+Z), which
        // is the same UX as repeated single-delete + undo.
        let mut deleted: usize = 0;
        let mut deleted_projects: usize = 0;
        for key in snapshot.iter() {
            if let NodeKey::Note(id) = key {
                let inverse = note_repo_for_bulk_delete.snapshot_subtree(*id).ok();
                match note_repo_for_bulk_delete.delete(*id) {
                    Ok(()) => {
                        deleted += 1;
                        if let Some(s) = inverse {
                            history.write().push(history::ExplorerAction::Delete {
                                snapshot: s,
                            });
                        }
                    }
                    Err(e) => eprintln!("operon: bulk delete note {id} failed: {e}"),
                }
            }
        }
        // Plans-Phase-4-multiselect-aria: delete project members. Done
        // after the note loop so project-row gymnastics (children flushed
        // first) match single-project delete semantics. No undo entry
        // pushed: the existing single-project delete path doesn't have a
        // snapshot/restore inverse either, so bulk parity is fine.
        for key in snapshot.iter() {
            if let NodeKey::Project(id) = key {
                match project_repo_for_bulk_project_delete.delete(*id) {
                    Ok(()) => {
                        deleted_projects += 1;
                        if *selected_project.read() == Some(*id) {
                            selected_project.set(None);
                        }
                    }
                    Err(e) => eprintln!("operon: bulk delete project {id} failed: {e}"),
                }
            }
        }
        if deleted_projects > 0 {
            project_version.with_mut(|v| *v += 1);
            note_version.with_mut(|v| *v += 1);
        }
        if deleted > 0 {
            note_version.with_mut(|v| *v += 1);
            // Close any open tabs for the deleted subtree(s).
            let to_close: Vec<crate::tabs::TabId> = {
                let snap = tabs_for_bulk_delete.read();
                snap.iter()
                    .filter(|t| deleted_note_ids.iter().any(|d| d == &t.note_id))
                    .map(|t| t.id)
                    .collect()
            };
            if !to_close.is_empty() {
                let mut tm = tabs_for_bulk_delete.write();
                for tid in to_close {
                    tm.close(tid);
                }
            }
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
                    other => {
                        let body = match persistence.load(&id.to_string()).await {
                            Ok(b) => b,
                            Err(_) => Vec::new(),
                        };
                        let ext = match other {
                            operon_store::repos::NoteKind::Mdx => "mdx",
                            operon_store::repos::NoteKind::Canvas => "canvas",
                            operon_store::repos::NoteKind::Excalidraw => "excalidraw",
                            operon_store::repos::NoteKind::Kanban => "kanban.json",
                            operon_store::repos::NoteKind::Code => "txt",
                            _ => "md",
                        };
                        let path = unique_path(&target, &safe_title, ext);
                        if std::fs::write(&path, &body).is_ok() {
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
            let (title, kind) = notes_by_project
                .read()
                .values()
                .flat_map(|list| list.iter())
                .find(|n| n.id == note_id)
                .map(|n| (n.title.clone(), n.kind))
                .unwrap_or_else(|| (note_id.to_string(), NoteKind::Markdown));
            open_local_note_tab(
                tabs_for_mode,
                scheduler_for_mode.clone(),
                note_id,
                title,
                String::new(),
                kind,
            )
        };
        tabs_for_mode.write().set_mode(tab_id, target);
        selected_note.set(Some(note_id));
        selected_project.set(None);
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

    // Plans-Phase-4-multiselect-aria: row-context bulk variants. Mirror the
    // keyboard handler in `LocalShellOverlay` (`desktop.rs`): when the
    // multi-set holds 2+ items, write a `BulkClipboard` and clear the
    // single-id clipboard so paste can disambiguate. Bulk-delete just
    // raises the existing confirmation flag — `on_confirm_bulk_delete`
    // already iterates the set.
    let mut bulk_clipboard_for_cut = bulk_clipboard;
    let mut single_clipboard_for_bulk_cut = clipboard;
    let multi_for_bulk_cut = multi_selected;
    let on_bulk_cut = use_callback(move |_: ()| {
        let items: Vec<ClipPayload> = multi_for_bulk_cut
            .read()
            .iter()
            .map(|k| match k {
                NodeKey::Note(id) => ClipPayload::Note(*id),
                NodeKey::Project(id) => ClipPayload::Project(*id),
            })
            .collect();
        if items.len() < 2 {
            return;
        }
        bulk_clipboard_for_cut.set(Some(crate::local_mode::ui::BulkClipboard {
            kind: ClipKind::Cut,
            items,
        }));
        single_clipboard_for_bulk_cut.set(None);
    });
    let mut bulk_clipboard_for_copy = bulk_clipboard;
    let mut single_clipboard_for_bulk_copy = clipboard;
    let multi_for_bulk_copy = multi_selected;
    let on_bulk_copy = use_callback(move |_: ()| {
        let items: Vec<ClipPayload> = multi_for_bulk_copy
            .read()
            .iter()
            .map(|k| match k {
                NodeKey::Note(id) => ClipPayload::Note(*id),
                NodeKey::Project(id) => ClipPayload::Project(*id),
            })
            .collect();
        if items.len() < 2 {
            return;
        }
        bulk_clipboard_for_copy.set(Some(crate::local_mode::ui::BulkClipboard {
            kind: ClipKind::Copy,
            items,
        }));
        single_clipboard_for_bulk_copy.set(None);
    });
    let mut bulk_delete_flag_setter = pending_bulk_delete_setter;
    let on_bulk_request_delete = use_callback(move |_: ()| {
        bulk_delete_flag_setter.set(true);
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
        // Plans-Phase-8-explorer-undo: capture the inverse before the
        // mutation. Cut + Note → MoveWithin (restore the source row's
        // pre-paste position); Copy + Note → Paste (delete the new root).
        let cut_inverse: Option<history::ExplorerAction> = match (clip.kind, clip.payload) {
            (ClipKind::Cut, ClipPayload::Note(nid)) => {
                let snap = notes_by_project.read();
                let mut found: Option<(Uuid, Option<Uuid>, i64)> = None;
                for (pid, list) in snap.iter() {
                    if let Some(n) = list.iter().find(|n| n.id == nid) {
                        found = Some((*pid, n.parent_id, n.sibling_index));
                        break;
                    }
                }
                found.map(|(pp, pparent, pidx)| history::ExplorerAction::MoveWithin {
                    id: nid,
                    project_id: pp,
                    prev_parent: pparent,
                    prev_index: pidx,
                })
            }
            _ => None,
        };
        let outcome: Result<Option<Uuid>, _> = match (clip.kind, clip.payload) {
            (ClipKind::Cut, ClipPayload::Note(nid)) => note_repo_for_paste
                .move_to(nid, project_id, Some(target), last_index)
                .map(|_| None),
            (ClipKind::Copy, ClipPayload::Note(nid)) => note_repo_for_paste
                .duplicate_subtree(nid, project_id, Some(target), last_index)
                .map(Some),
            (_, ClipPayload::Project(_)) => Ok(None),
        };
        match outcome {
            Ok(new_root) => {
                if let Some(inverse) = cut_inverse {
                    history.write().push(inverse);
                } else if let Some(rid) = new_root {
                    history
                        .write()
                        .push(history::ExplorerAction::Paste { pasted_root_id: rid });
                }
            }
            Err(e) => {
                eprintln!("operon: paste-into-note failed: {e}");
                return;
            }
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
        // Plans-Phase-8-explorer-undo: same shape as on_paste_into_note —
        // Cut → MoveWithin inverse; Copy → Paste inverse.
        let cut_inverse: Option<history::ExplorerAction> = match (clip.kind, clip.payload) {
            (ClipKind::Cut, ClipPayload::Note(nid)) => {
                let snap = notes_by_project.read();
                let mut found: Option<(Uuid, Option<Uuid>, i64)> = None;
                for (pid, list) in snap.iter() {
                    if let Some(n) = list.iter().find(|n| n.id == nid) {
                        found = Some((*pid, n.parent_id, n.sibling_index));
                        break;
                    }
                }
                found.map(|(pp, pparent, pidx)| history::ExplorerAction::MoveWithin {
                    id: nid,
                    project_id: pp,
                    prev_parent: pparent,
                    prev_index: pidx,
                })
            }
            _ => None,
        };
        let outcome: Result<Option<Uuid>, _> = match (clip.kind, clip.payload) {
            (ClipKind::Cut, ClipPayload::Note(nid)) => note_repo_for_paste_proj
                .move_to(nid, target_project, None, last_index)
                .map(|_| None),
            (ClipKind::Copy, ClipPayload::Note(nid)) => note_repo_for_paste_proj
                .duplicate_subtree(nid, target_project, None, last_index)
                .map(Some),
            (_, ClipPayload::Project(_)) => Ok(None),
        };
        match outcome {
            Ok(new_root) => {
                if let Some(inverse) = cut_inverse {
                    history.write().push(inverse);
                } else if let Some(rid) = new_root {
                    history
                        .write()
                        .push(history::ExplorerAction::Paste { pasted_root_id: rid });
                }
            }
            Err(e) => {
                eprintln!("operon: paste-into-project failed: {e}");
                return;
            }
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
            // Plans-Phase-8-explorer-undo: capture src's pre-move position.
            let inverse = {
                let snap = notes_by_project.read();
                snap.iter().find_map(|(pid, list)| {
                    list.iter().find(|n| n.id == src).map(|n| {
                        history::ExplorerAction::MoveWithin {
                            id: src,
                            project_id: *pid,
                            prev_parent: n.parent_id,
                            prev_index: n.sibling_index,
                        }
                    })
                })
            };
            let last_index = note_repo_for_drop_pr
                .list_for_project(target)
                .map(|rows| rows.iter().filter(|r| r.parent_id.is_none()).count() as i64)
                .unwrap_or(0);
            if let Err(e) = note_repo_for_drop_pr.move_to(src, target, None, last_index) {
                eprintln!("operon: drop note onto project failed: {e}");
                return;
            }
            if let Some(inv) = inverse {
                history.write().push(inv);
            }
            note_version.with_mut(|v| *v += 1);
        });

    // Drag-drop: note onto note. Before/After → sibling at target.parent_id;
    // Into → child of target.
    let note_repo_for_drop_n = note_repo.clone();
    let on_drop_note_on_note = use_callback(
        move |(src, target, pos, chosen_depth): (Uuid, Uuid, DropPosition, i64)| {
            if src == target {
                return;
            }
            // Plans-Phase-8-explorer-undo: capture src's pre-move position.
            let src_inverse = {
                let snap = notes_by_project.read();
                snap.iter().find_map(|(pid, list)| {
                    list.iter().find(|n| n.id == src).map(|n| {
                        history::ExplorerAction::MoveWithin {
                            id: src,
                            project_id: *pid,
                            prev_parent: n.parent_id,
                            prev_index: n.sibling_index,
                        }
                    })
                })
            };
            // Look up target's project and full row so the depth-aware
            // resolver can walk its ancestor chain. We snapshot the per-
            // project list once because resolve_drop_parent needs it for
            // outdent steps.
            let lookup = notes_by_project.read().iter().find_map(|(pid, list)| {
                list.iter()
                    .find(|n| n.id == target)
                    .cloned()
                    .map(|n| (*pid, list.clone(), n))
            });
            let Some((project_id, project_notes, target_note)) = lookup else {
                return;
            };
            let (new_parent, new_index) =
                resolve_drop_parent(&target_note, pos, chosen_depth, &project_notes);
            let outcome = note_repo_for_drop_n.move_to(src, project_id, new_parent, new_index);
            if let Err(e) = outcome {
                eprintln!("operon: drop note onto note failed: {e}");
                return;
            }
            if let Some(inv) = src_inverse {
                history.write().push(inv);
            }
            note_version.with_mut(|v| *v += 1);
        },
    );

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
    // signal so Shift+click can compute true ranges. Computed via use_memo
    // (read-driven, fires reliably even when sibling use_effects don't),
    // then synced into the context Signal so child rows can read it. The
    // peek+set guard avoids infinite render loops when the computed value
    // is unchanged.
    let visible_flat: Signal<Vec<NodeKey>> = use_context::<VisibleFlat>().0;
    let mut visible_flat_setter = visible_flat;
    let visible_flat_memo: Memo<Vec<NodeKey>> = use_memo(move || {
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
        out
    });
    // Sync inline on every render so shift+click sees a fresh list even
    // if the use_effect path is delayed. The peek+set guard prevents
    // re-render loops when the value is unchanged.
    {
        let next = visible_flat_memo.read().clone();
        if visible_flat_setter.peek().clone() != next {
            visible_flat_setter.set(next);
        }
    }

    // ===== Render =====
    let projects_snapshot = projects.read().clone();
    let renaming_project_now = *renaming_project.read();
    let renaming_note_now = *renaming_note.read();
    let selected_project_now = *selected_project.read();
    let selected_note_now = *selected_note.read();

    // Search-mode flag — lifted up so the heavy tree-only snapshots below
    // can short-circuit when search results are showing. Without this, every
    // keystroke re-renders the panel and runs O(notes_total) clones / walks
    // for data the search-results branch never reads.
    let debounced_now = debounced_query.read().clone();
    let show_results = !debounced_now.trim().is_empty();

    // Derive the note (and its enclosing project) currently bound to the
    // active tab so the explorer can render a distinct "this is open in
    // the active editor" highlight separate from explorer click-selection.
    // Memoized so unrelated panel re-renders (search keystrokes, multi-
    // select, drag-session, etc.) don't reread `tabs` / `notes_by_project`.
    let active_tab_note: Memo<Option<Uuid>> = use_memo(move || {
        tabs.read()
            .active()
            .and_then(|t| Uuid::parse_str(&t.note_id).ok())
    });
    let active_tab_project: Memo<Option<Uuid>> = use_memo(move || {
        let nid = (*active_tab_note.read())?;
        notes_by_project
            .read()
            .iter()
            .find_map(|(pid, list)| list.iter().any(|n| n.id == nid).then_some(*pid))
    });
    // Set of note UUIDs whose tabs are currently dirty (unsaved). The
    // explorer renders a leading dot on rows in this set so users can
    // see at a glance which notes have pending edits — same dirty
    // signal that drives the tab strip's circle-dot marker, just
    // surfaced in the tree as well.
    let dirty_note_ids: Memo<std::collections::HashSet<Uuid>> = use_memo(move || {
        tabs.read()
            .iter()
            .filter(|t| t.dirty)
            .filter_map(|t| Uuid::parse_str(&t.note_id).ok())
            .collect()
    });
    // Per-Artifact-note frontmatter cache. One sync `block_on` load
    // per Artifact populates both the SDLC kind (drives the role chip
    // + title/caret tint) and the status (drives the right-aligned
    // status dot). Re-runs whenever `notes_by_project` changes.
    // Synchronous reads are fine at local-mode scale; for hundreds of
    // artifacts we'd move this to an async cache.
    let persistence_for_kinds = persistence.clone();
    let artifact_meta: Memo<HashMap<Uuid, role::ArtifactMeta>> = use_memo(move || {
        let mut map = HashMap::new();
        let snapshot = notes_by_project.read();
        for (_, list) in snapshot.iter() {
            for note in list.iter() {
                if !matches!(note.kind, operon_store::repos::NoteKind::Artifact) {
                    continue;
                }
                let id = note.id;
                let body =
                    futures::executor::block_on(persistence_for_kinds.load(&id.to_string()));
                if let Ok(bytes) = body {
                    if let Ok(s) = String::from_utf8(bytes) {
                        let fm = crate::plugins::artifact::frontmatter::parse(&s);
                        map.insert(
                            id,
                            role::ArtifactMeta {
                                kind: fm.artifact_kind,
                                status: fm.status,
                                needs_review: fm.needs_review,
                            },
                        );
                    }
                }
            }
        }
        map
    });

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
    let mut tabs_for_delete = tabs;
    // Snapshot the per-note current editor mode from the tab manager so each
    // NoteRow can show the right context-menu items (View / Edit / Split).
    // Skipped in search-results mode — the tree (and its NoteRows) is not
    // rendered, so the snapshot would be pure waste on every keystroke.
    let note_modes_snapshot: HashMap<Uuid, EditorMode> = if show_results {
        HashMap::new()
    } else {
        let snap = tabs.read();
        snap.iter()
            .filter_map(|t| Uuid::parse_str(&t.note_id).ok().map(|u| (u, t.mode)))
            .collect()
    };
    // Tree-only snapshots — full clones of the workspace/notes maps that the
    // search-results branch never reads. Gated on `show_results` so typing
    // doesn't churn through O(notes_total) clones per keystroke.
    let (workspace_snap, project_note_open_snap, notes_snap) = if show_results {
        (HashMap::new(), HashMap::new(), HashMap::new())
    } else {
        (
            workspace_open.read().clone(),
            project_note_open.read().clone(),
            notes_by_project.read().clone(),
        )
    };
    let clipboard_snap = *clipboard.read();
    let drag_session_now = *drag_session.read();
    let has_clip_note = matches!(
        clipboard_snap,
        Some(Clipboard {
            payload: ClipPayload::Note(_),
            ..
        })
    );

    // Title + kind lookup for the search-result click handler. The kind
    // half drives `open_local_note_tab`'s format_id derivation so opening
    // a non-markdown note from search lands the user in the right editor.
    let note_meta_signal: Signal<HashMap<Uuid, (String, NoteKind)>> = use_signal(HashMap::new);
    {
        let mut meta_setter = note_meta_signal;
        use_effect(move || {
            let map = notes_by_project.read();
            let mut out: HashMap<Uuid, (String, NoteKind)> = HashMap::new();
            for list in map.values() {
                for n in list {
                    out.insert(n.id, (n.title.clone(), n.kind));
                }
            }
            meta_setter.set(out);
        });
    }
    let on_search_pick = search_click_handler(
        tabs,
        save_scheduler.clone(),
        selected_note,
        selected_project,
        workspace_open,
        tree_queue,
        note_meta_signal,
        persistence.clone(),
        search_query,
    );

    // Esc inside the search input clears query + restores focus to prev_selection.
    let prev_selection_setter = prev_selection;
    let mut search_query_setter = search_query;
    let mut selected_note_for_clear = selected_note;
    let on_search_clear = use_callback(move |_| {
        search_query_setter.set(String::new());
        if let Some(prev) = *prev_selection_setter.read() {
            selected_note_for_clear.set(Some(prev));
        }
    });

    // Capture prev_selection on transition from empty -> non-empty query.
    // peek() (not read()) on prev_setter and selected_note so the effect
    // only re-runs when search_query changes — without this, writing to
    // prev_setter inside the effect re-triggers the effect through its
    // own subscription, looping forever whenever selected_note is None
    // (the .set(None) on a None signal still wakes subscribers).
    {
        let mut prev_setter = prev_selection;
        use_effect(move || {
            let q = search_query.read().clone();
            let prev_is_some = prev_setter.peek().is_some();
            if !q.is_empty() && !prev_is_some {
                prev_setter.set(*selected_note.peek());
            } else if q.is_empty() && prev_is_some {
                prev_setter.set(None);
            }
        });
    }

    rsx! {
        div {
            class: "notes-explorer-list",
            "data-testid": "explorer-panel",
            "data-explorer-root": "true",
            tabindex: "0",
            // Plans-Phase-4-explorer-undo-stack: explorer-scoped Cmd/Ctrl+Z.
            // Scope check is implicit — the listener only fires for events
            // whose target is inside this div, so the editor's intrinsic
            // undo (Monaco) is left alone when focus is in the editor.
            // Plans-Phase-10: also skip while the user is mid-rename so the
            // input keeps its intrinsic text-undo. Otherwise pressing
            // Cmd+Z while typing the title of a freshly-created note
            // would yank the row out from under them.
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                let mods = evt.modifiers();
                let with_meta = mods.contains(keyboard_types::Modifiers::META)
                    || mods.contains(keyboard_types::Modifiers::CONTROL);
                if with_meta
                    && key.eq_ignore_ascii_case("z")
                    && renaming_note.read().is_none()
                {
                    evt.prevent_default();
                    evt.stop_propagation();
                    if mods.contains(keyboard_types::Modifiers::SHIFT) {
                        // Plans-Phase-11: Cmd/Ctrl+Shift+Z → redo.
                        on_redo.call(());
                    } else {
                        on_undo.call(());
                    }
                }
            },
            style: "list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; flex: 1; min-height: 0;",
            // Header — search input + checkbox + "+" button. Borderless to match
            // Cloud's notes-explorer chrome density.
            div {
                class: "notes-explorer-toolbar",
                div {
                    class: "notes-explorer-toolbar-search",
                    ExplorerSearch {
                        query: search_query,
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
                    on_pick: on_search_pick,
                }
            } else {
            div {
                class: "flex-1 overflow-y-auto",
                style: "min-height: 0;",
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
                    if key == "Escape" {
                        // Clear all selection state — single + multi + anchor
                        // + focused-row marker. Stop propagation so a wrapping
                        // shell (palette, etc.) doesn't also eat this Escape.
                        evt.prevent_default();
                        evt.stop_propagation();
                        if !multi_selected.peek().is_empty() {
                            multi_selected.set(std::collections::BTreeSet::new());
                        }
                        if selected_note.peek().is_some() {
                            selected_note.set(None);
                        }
                        if selected_project.peek().is_some() {
                            selected_project.set(None);
                        }
                        if last_clicked_for_clear.peek().is_some() {
                            last_clicked_for_clear.set(None);
                        }
                        if focused_node_for_clear.peek().is_some() {
                            focused_node_for_clear.set(None);
                        }
                    } else if key == "Delete" || key == "Backspace" {
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
                if let Some(err) = project_load_error.read().as_ref() {
                    div {
                        class: "px-3 py-6 text-xs text-rose-500 text-center",
                        "data-testid": "explorer-load-error",
                        role: "alert",
                        "{err}"
                    }
                } else if projects_snapshot.is_empty() {
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
                            active_tab_note: *active_tab_note.read(),
                            active_tab_project: *active_tab_project.read(),
                            dirty_note_ids: dirty_note_ids.read().clone(),
                            artifact_meta: artifact_meta.read().clone(),
                            clipboard: clipboard_snap,
                            has_clip_note: has_clip_note,
                            drag_session: drag_session_now,
                            on_select_project: on_select_project,
                            on_rename_project: on_rename_project,
                            on_delete_project: on_delete_project_noop,
                            on_request_rename_project: on_request_rename_project,
                            on_request_delete_project: on_request_delete_project,
                            on_toggle_project: on_toggle_project,
                            on_add_project_note: on_add_project_note,
                            on_add_project_phase: on_add_project_phase,
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
                            on_bulk_cut: on_bulk_cut,
                            on_bulk_copy: on_bulk_copy,
                            on_bulk_request_delete: on_bulk_request_delete,
                            on_set_repo_path: on_set_repo_path,
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
                    // since the FK cascade will lose them. Same walk also
                    // collects the note ids whose tabs we must close after
                    // a successful delete.
                    let mut blobs_to_check: Vec<String> = Vec::new();
                    let mut deleted_note_ids: Vec<String> = vec![did.to_string()];
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
                                    deleted_note_ids.push(n.id.to_string());
                                    break;
                                }
                                cur = list.iter().find(|x| x.id == pid).and_then(|x| x.parent_id);
                            }
                        }
                    }
                    drop(snap);
                    // Plans-Phase-8-explorer-undo: snapshot the subtree before
                    // delete so undo can re-INSERT it. Capture failure is
                    // non-fatal (we still delete; the user just can't undo).
                    let undo_snapshot =
                        match note_repo_for_delete.snapshot_subtree(did) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                eprintln!(
                                    "operon: snapshot before delete failed: {e}"
                                );
                                None
                            }
                        };
                    match note_repo_for_delete.delete(did) {
                        Ok(()) => {
                            if let Some(snapshot) = undo_snapshot {
                                history
                                    .write()
                                    .push(history::ExplorerAction::Delete { snapshot });
                            }
                            note_version.with_mut(|v| *v += 1);
                            if selected_note_now == Some(did) {
                                selected_note.set(None);
                            }
                            // Close any open tabs that referenced the deleted
                            // subtree — multiple tabs can target the same
                            // note_id (View / Edit / Split), so collect-then-
                            // close to avoid mutating during iteration.
                            let to_close: Vec<crate::tabs::TabId> = {
                                let snap = tabs_for_delete.read();
                                snap.iter()
                                    .filter(|t| deleted_note_ids.iter().any(|d| d == &t.note_id))
                                    .map(|t| t.id)
                                    .collect()
                            };
                            if !to_close.is_empty() {
                                let mut tm = tabs_for_delete.write();
                                for tid in to_close {
                                    tm.close(tid);
                                }
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
    /// Note id bound to the currently active tab (if any). NoteRow uses this
    /// to render an accent indicating "this note is what the editor shows".
    active_tab_note: Option<Uuid>,
    /// Project that contains `active_tab_note`. ProjectRow uses this to
    /// mirror the same accent at the project level.
    active_tab_project: Option<Uuid>,
    /// Note ids whose open tabs are dirty (unsaved). NoteRow renders a
    /// leading dot for rows in this set.
    dirty_note_ids: std::collections::HashSet<Uuid>,
    /// Artifact-frontmatter snapshot used to drive each row's SDLC
    /// role chip (BA / SA / SDE) and the right-aligned status dot.
    /// Populated upstream by parsing each Artifact note's frontmatter.
    /// Skill notes derive their role from the title's numeric prefix
    /// and don't consult this map.
    artifact_meta: HashMap<Uuid, role::ArtifactMeta>,
    clipboard: Option<Clipboard>,
    has_clip_note: bool,
    drag_session: Option<DragKind>,
    on_select_project: Callback<Uuid>,
    on_rename_project: Callback<(Uuid, String)>,
    on_delete_project: Callback<Uuid>,
    on_request_rename_project: Callback<Uuid>,
    on_request_delete_project: Callback<Uuid>,
    on_toggle_project: Callback<Uuid>,
    on_add_project_note: Callback<(Uuid, CreatableKind)>,
    on_add_project_phase: Callback<Uuid>,
    on_drop_image_into_note: Callback<(Uuid, Vec<u8>, String)>,
    on_drop_image_into_project: Callback<(Uuid, Vec<u8>, String)>,
    on_select_note: Callback<Uuid>,
    on_toggle_note: Callback<(Uuid, Uuid)>,
    on_rename_note: Callback<(Uuid, String)>,
    on_request_rename_note: Callback<Uuid>,
    on_request_delete_note: Callback<Uuid>,
    on_add_child_note: Callback<(Uuid, CreatableKind)>,
    on_add_sibling_note: Callback<(Uuid, CreatableKind)>,
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
    on_drop_note_on_note: Callback<(Uuid, Uuid, DropPosition, i64)>,
    note_modes: HashMap<Uuid, EditorMode>,
    on_set_note_mode: Callback<(Uuid, EditorMode)>,
    /// Plans-Phase-4-multiselect-aria: row-context bulk variants. The row
    /// component decides whether to fire these (when its id is in
    /// `MultiSelected` and the set has 2+ items) or the per-id callbacks.
    on_bulk_cut: Callback<()>,
    on_bulk_copy: Callback<()>,
    on_bulk_request_delete: Callback<()>,
    /// M1-companion-claude-code: bind / clear the project's git repository.
    on_set_repo_path: Callback<(Uuid, Option<std::path::PathBuf>)>,
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

    // Only flag the project as tab-active when the active note's row is NOT
    // currently visible (project collapsed, or hidden behind a collapsed
    // ancestor). When the note row is visible, its own accent is enough —
    // duplicating it on the parent reads as one merged block instead of
    // two distinct rows.
    let active_note_visible = props
        .active_tab_note
        .map(|nid| visible.iter().any(|n| n.id == nid))
        .unwrap_or(false);
    let project_tab_active =
        props.active_tab_project == Some(project_id) && !active_note_visible;

    rsx! {
        ProjectRow {
            project: props.project.clone(),
            is_open: props.is_open,
            selected: props.selected,
            tab_active: project_tab_active,
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
            on_add_note: props.on_add_project_note,
            on_add_phase: props.on_add_project_phase,
            on_drop_image_file: props.on_drop_image_into_project,
            on_cut: props.on_cut_project,
            on_copy: props.on_copy_project,
            on_paste: props.on_paste_into_project,
            on_drop_project_on_project: props.on_drop_project_on_project,
            on_drop_note_on_project: props.on_drop_note_on_project,
            on_bulk_cut: props.on_bulk_cut,
            on_bulk_copy: props.on_bulk_copy,
            on_bulk_request_delete: props.on_bulk_request_delete,
            on_set_repo_path: props.on_set_repo_path,
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
                        tab_active: props.active_tab_note == Some(note.id),
                        dirty: props.dirty_note_ids.contains(&note.id),
                        role: match note.kind {
                            operon_store::repos::NoteKind::Artifact => props
                                .artifact_meta
                                .get(&note.id)
                                .and_then(|m| m.kind.as_ref())
                                .and_then(role::role_for_artifact_kind),
                            operon_store::repos::NoteKind::Skill => {
                                role::role_for_skill_title(&note.title)
                            }
                            _ => None,
                        },
                        artifact_status: props
                            .artifact_meta
                            .get(&note.id)
                            .map(|m| m.status),
                        needs_review: props
                            .artifact_meta
                            .get(&note.id)
                            .map(|m| m.needs_review)
                            .unwrap_or(false),
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
                        on_bulk_cut: props.on_bulk_cut,
                        on_bulk_copy: props.on_bulk_copy,
                        on_bulk_request_delete: props.on_bulk_request_delete,
                    }
                }
            }
        }
    }
}
