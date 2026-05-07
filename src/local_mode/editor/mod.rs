//! Editor surface for Local-Mode note tabs.
//!
//! Open a `LocalNote` -> a `Tab` in the shared `TabManager` with `manual_save = true`.
//! Saving runs through the bytes-only `Persistence` trait (the local-mode root is
//! `<store>/local/`); after a successful save we bump `local_note.updated_at_ms`
//! via the repo and clear `Tab.dirty`.
//!
//! The plan note pointed at `NoteHub::open(note_id)` for content storage; that
//! integration requires a local-mode-aware `NoteRepository` (the existing one is
//! cloud-side, keyed by `notes.id`). Threading that through Phase-3 is out of
//! scope, so we keep the existing `Persistence` trait for now and leave the
//! NoteHub wiring as a Phase-4 follow-up. From the user's perspective the save
//! contract is identical (explicit via button + Ctrl+S, dirty indicator).

use std::sync::Arc;

use dioxus::html::HasFileData;
use dioxus::prelude::*;
use operon_store::repos::{
    LinkRow, LocalNoteLinkRepository, LocalNoteRepository, LocalProjectRepository,
};
use operon_store::vfs;
use uuid::Uuid;

use crate::editor::{EditorMode, LanguageDescriptor};
use crate::persistence::Persistence;
use crate::plugins::markdown::wikilink;
use crate::shell::editor_host::{MonacoChannel, MonacoEditorHost};
use crate::tabs::{SaveScheduler, Tab, TabId, TabManager};

mod link_picker;
pub use link_picker::{LinkPicker, PickedLink};

/// Shared callback installed at LocalShell scope. The keyboard handler at the
/// shell root (Ctrl+S) and the explicit Save button both invoke this. It looks
/// up the active tab, snapshots its content, persists it, and bumps the
/// `local_note.updated_at_ms` row.
#[derive(Clone, PartialEq)]
pub struct LocalSaveAction {
    pub callback: Callback<()>,
}

/// Mount the explicit-save action at LocalShell scope. The returned callback
/// is what `Ctrl+S` and the Save button invoke.
///
/// Plans-Phase-2-saving: routes through [`SaveScheduler::flush`] so the
/// manual save and the 150 ms debounced autosave converge on a single
/// `Persistence::save` call site. Calling `flush` first cancels any
/// in-flight debounce future for the tab so we never double-write.
///
/// Plans-Phase-5-vfs-wikilinks: after a successful save, parses the body
/// via `wikilink::extract_links`, resolves each target via
/// `vfs::resolve_link`, and rebuilds the `local_note_link` rows for this
/// source via `LocalNoteLinkRepository::replace_for`.
pub fn install_save_action(
    mut tabs: Signal<TabManager>,
    _persistence: Arc<dyn Persistence>,
    note_repo: Arc<dyn LocalNoteRepository>,
    project_repo: Arc<dyn LocalProjectRepository>,
    link_repo: Arc<dyn LocalNoteLinkRepository>,
    scheduler: SaveScheduler,
) -> Callback<()> {
    Callback::new(move |_| {
        let active: Option<Tab> = tabs.read().active().cloned();
        let Some(tab) = active else {
            return;
        };
        if !tab.manual_save {
            return;
        }
        let Ok(note_uuid) = Uuid::parse_str(&tab.note_id) else {
            eprintln!(
                "operon: local-save called on non-uuid note_id {}",
                tab.note_id
            );
            return;
        };
        let tab_id = tab.id;
        let note_id = tab.note_id.clone();
        let content = tab.content.clone();
        let repo = note_repo.clone();
        let project_repo = project_repo.clone();
        let link_repo = link_repo.clone();
        let scheduler = scheduler.clone();

        // Plans-Phase-9-monaco-desktop (rev 17): probe to confirm
        // Cmd/Ctrl+S actually reaches the save callback. If we never
        // see this line in stderr after pressing Save, the keyaction
        // chain (Monaco bootstrap → recv loop → on_action handler →
        // action.callback) is broken upstream. Removed in cleanup.
        eprintln!(
            "operon: save fired note_id={note_id} len={} dirty={}",
            content.len(),
            tab.dirty
        );

        spawn(async move {
            match scheduler.flush(tab_id, &note_id, &content).await {
                Ok(()) => {
                    eprintln!("operon: save flushed note_id={note_id} len={}", content.len());
                    if let Err(e) = repo.touch_updated(note_uuid) {
                        eprintln!("operon: touch_updated failed for {note_uuid}: {e}");
                    }
                    rebuild_link_graph_for_source(
                        note_uuid,
                        &content,
                        &project_repo,
                        &repo,
                        &link_repo,
                    );
                    tabs.write().set_dirty(tab_id, false);
                }
                Err(e) => {
                    eprintln!("operon: local note save failed: {e}");
                }
            }
        });
    })
}

/// Plans-Phase-5-vfs-wikilinks: extract every wikilink in `body`, resolve
/// each against the source-note's project (best-effort), and replace the
/// `local_note_link` rows for the source. Resolution failures are kept as
/// rows with `target_note_id = NULL` so the renderer still surfaces them
/// as broken.
pub fn rebuild_link_graph_for_source(
    source_id: Uuid,
    body: &str,
    project_repo: &Arc<dyn LocalProjectRepository>,
    note_repo: &Arc<dyn LocalNoteRepository>,
    link_repo: &Arc<dyn LocalNoteLinkRepository>,
) {
    let extracted = wikilink::extract_links(body);
    if extracted.is_empty() {
        if let Err(e) = link_repo.replace_for(source_id, &[]) {
            eprintln!("operon: rebuild_link_graph clear failed: {e}");
        }
        return;
    }
    // Plans-Phase-9-wikilink-picker (rev 3): snapshot projects + every
    // project's notes ONCE up front, then resolve all extracted links
    // against the in-memory snapshot. Calling `vfs::resolve_link` per
    // link issued one `list()` and at least one `list_for_project()`
    // for every wikilink — for a body with N links and P projects that
    // was N*(1 + P) DB queries on the main thread, plenty enough to
    // freeze the UI on a paste of a moderately-linked note.
    let projects = project_repo.list().unwrap_or_default();
    let mut notes_by_project: std::collections::HashMap<Uuid, Vec<operon_store::repos::LocalNote>> =
        std::collections::HashMap::new();
    for p in &projects {
        if let Ok(rows) = note_repo.list_for_project(p.id) {
            notes_by_project.insert(p.id, rows);
        }
    }
    let source_project_id = notes_by_project
        .iter()
        .find_map(|(pid, notes)| notes.iter().find(|n| n.id == source_id).map(|_| *pid));
    let resolve = |form: &vfs::LinkForm| -> Option<Uuid> {
        match form {
            vfs::LinkForm::Relative { title } => {
                let spid = source_project_id?;
                let notes = notes_by_project.get(&spid)?;
                let matches: Vec<Uuid> = notes
                    .iter()
                    .filter(|n| n.title.eq_ignore_ascii_case(title))
                    .map(|n| n.id)
                    .collect();
                if matches.len() == 1 {
                    Some(matches[0])
                } else {
                    None
                }
            }
            vfs::LinkForm::Absolute { project, title } => {
                let pid = projects
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(project))?
                    .id;
                let notes = notes_by_project.get(&pid)?;
                let matches: Vec<Uuid> = notes
                    .iter()
                    .filter(|n| n.title.eq_ignore_ascii_case(title))
                    .map(|n| n.id)
                    .collect();
                if matches.len() == 1 {
                    Some(matches[0])
                } else {
                    None
                }
            }
            vfs::LinkForm::Nested {
                project,
                parent_path,
                title,
                short_id,
            } => {
                let pid = projects
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(project))?
                    .id;
                let notes = notes_by_project.get(&pid)?;
                let mut frontier: Vec<Uuid> = notes
                    .iter()
                    .filter(|n| n.parent_id.is_none())
                    .map(|n| n.id)
                    .collect();
                for segment in parent_path {
                    let next: Vec<Uuid> = notes
                        .iter()
                        .filter(|n| {
                            n.parent_id
                                .map(|p| frontier.contains(&p))
                                .unwrap_or(false)
                                && n.title.eq_ignore_ascii_case(segment)
                        })
                        .map(|n| n.id)
                        .collect();
                    if next.is_empty() {
                        return None;
                    }
                    frontier = next;
                }
                let matches: Vec<Uuid> = notes
                    .iter()
                    .filter(|n| {
                        let parent_ok = if parent_path.is_empty() {
                            n.parent_id.is_none()
                        } else {
                            n.parent_id
                                .map(|p| frontier.contains(&p))
                                .unwrap_or(false)
                        };
                        let title_ok = n.title.eq_ignore_ascii_case(title);
                        let short_ok = match short_id {
                            Some(s) => n.id.simple().to_string().starts_with(s),
                            None => true,
                        };
                        parent_ok && title_ok && short_ok
                    })
                    .map(|n| n.id)
                    .collect();
                if matches.len() == 1 {
                    Some(matches[0])
                } else {
                    None
                }
            }
            vfs::LinkForm::Disambiguated { title, short_id } => {
                let mut matches: Vec<Uuid> = Vec::new();
                for notes in notes_by_project.values() {
                    for n in notes {
                        if n.title.eq_ignore_ascii_case(title)
                            && n.id.simple().to_string().starts_with(short_id)
                        {
                            matches.push(n.id);
                        }
                    }
                }
                if matches.len() == 1 {
                    Some(matches[0])
                } else {
                    None
                }
            }
        }
    };

    let mut rows: Vec<LinkRow> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for link in extracted {
        if !seen.insert(link.target.clone()) {
            // (source, target) is the PK; keep the first occurrence's
            // is_embed flag and skip duplicates.
            continue;
        }
        let target_note_id = vfs::parse_link(&link.target).and_then(|form| resolve(&form));
        rows.push(LinkRow {
            source_note_id: source_id,
            target_text: link.target,
            target_note_id,
            is_embed: link.embed,
        });
    }
    if let Err(e) = link_repo.replace_for(source_id, &rows) {
        eprintln!("operon: rebuild_link_graph replace_for failed: {e}");
    }
}

/// Open (or focus) a Local-Mode note tab for `note_uuid`. The tab uses the
/// `manual_save = true` path so the debounced autosave never fires.
///
/// Plans-Phase-9-monaco-desktop (rev 14): "click on a note in the
/// explorer" semantics:
/// - If an Edit-mode tab for the note already exists, focus it.
/// - If a non-Edit (View / Split / LivePreview) tab exists for the
///   note, leave it alone and open a *new* Edit tab alongside.
/// - If no tab exists, open a new Edit tab.
///
/// The intent: the user can keep a View / Split tab open as a
/// reference and click the note again to get a fresh editable
/// buffer in a new tab. Each tab carries its own buffer; saves go
/// through the same `Persistence` row keyed by note id, so
/// last-write-wins on the SQLite side.
pub fn open_local_note_tab(
    mut tabs: Signal<TabManager>,
    save_scheduler: crate::tabs::SaveScheduler,
    note_uuid: Uuid,
    title: String,
    initial_content: String,
    kind: operon_store::repos::NoteKind,
) -> TabId {
    let note_id_str = note_uuid.to_string();
    // Look for an existing Edit-mode tab to focus.
    let existing_edit = {
        let snap = tabs.read();
        let id = snap
            .iter()
            .find(|t| t.note_id == note_id_str && matches!(t.mode, EditorMode::Edit))
            .map(|t| t.id);
        id
    };
    if let Some(tid) = existing_edit {
        tabs.write().activate(tid);
        return tid;
    }
    // No Edit-mode tab — force a new tab so View / Split tabs
    // remain undisturbed. format_id is derived from the note's kind so
    // MainArea can dispatch to the right FormatPlugin (markdown → textarea
    // fallback inside LocalNoteEditor; mdx/canvas/excalidraw/kanban/code →
    // their own plugin's render_edit).
    let id = tabs.write().open_manual_save_new(
        note_id_str,
        kind.format_id().to_string(),
        title,
        initial_content,
    );
    save_scheduler.set_manual_save(id);
    tabs.write().set_mode(id, EditorMode::Edit);
    id
}

/// Image-note dispatch: returns the image viewer when the note has an
/// attached blob, an empty-state pane (with a file-picker button) when
/// the row exists but `blob_path` is `None`, or `None` when the note
/// isn't an image / the row can't be found.
///
/// The empty-state branch is the entry point for the "create image
/// placeholder, attach file later" flow that Operon-Phase-3 introduced.
///
/// NOTE: this function is dead since image notes now flow through the
/// `ImageFormatPlugin` registered in `plugin::registry`. Kept around for
/// the unit tests of `splice_at_caret` below; remove once the move is
/// settled.
#[allow(dead_code)]
fn try_render_image_view(
    note_id: Uuid,
    note_repo_handle: &crate::local_mode::desktop::LocalNoteRepo,
    project_repo_handle: &crate::local_mode::desktop::LocalProjectRepo,
    vault_handle: &crate::local_mode::desktop::CurrentVaultRoot,
) -> Option<Element> {
    use operon_store::repos::NoteKind;

    let crate::local_mode::desktop::LocalNoteRepo(note_repo) = note_repo_handle;
    let crate::local_mode::desktop::LocalProjectRepo(project_repo) = project_repo_handle;
    let crate::local_mode::desktop::CurrentVaultRoot(vault_signal) = vault_handle;
    let vault = vault_signal.read().clone()?;

    let projects = project_repo.list().ok()?;
    let row = projects.into_iter().find_map(|p| {
        note_repo
            .list_for_project(p.id)
            .ok()?
            .into_iter()
            .find(|n| n.id == note_id)
    })?;

    if !matches!(row.kind, NoteKind::Image) {
        return None;
    }

    // Empty-state branch. Image note exists but no blob attached yet.
    if row.blob_path.is_none() {
        let note_repo_for_pick = note_repo.clone();
        let vault_for_pick = vault.clone();
        let crate::local_mode::explorer::LocalNoteVersion(mut note_version) =
            use_context::<crate::local_mode::explorer::LocalNoteVersion>();
        let pick_image = move || {
            let note_repo = note_repo_for_pick.clone();
            let vault = vault_for_pick.clone();
            spawn(async move {
                let Some(handle) = rfd::AsyncFileDialog::new()
                    .set_title("Choose an image")
                    .add_filter(
                        "Image",
                        &["png", "jpg", "jpeg", "webp", "gif", "svg", "avif"],
                    )
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
                        eprintln!("operon: image picker: unsupported extension {ext}");
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
                let rel = written.relative_path.to_string_lossy().to_string();
                if let Err(e) = note_repo.set_blob_path(note_id, Some(&rel)) {
                    eprintln!("operon: set_blob_path failed: {e}");
                    return;
                }
                // Bump note_version so the explorer + editor re-read the
                // row and swap the empty-state pane for the viewer.
                note_version.with_mut(|v| *v += 1);
            });
        };
        return Some(rsx! {
            div {
                class: "operon-local-image-empty",
                "data-testid": "image-note-empty",
                "data-note-id": "{note_id}",
                style: "display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.75rem; height: 100%; padding: 2rem; text-align: center; color: var(--operon-fg, #ddd); background: var(--operon-bg, #111);",
                p {
                    style: "margin: 0; opacity: 0.75;",
                    "No image attached yet."
                }
                button {
                    r#type: "button",
                    class: "operon-button",
                    "data-testid": "image-note-pick-button",
                    style: "padding: 0.5rem 1rem; border-radius: 0.25rem; cursor: pointer;",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        pick_image();
                    },
                    "Choose image…"
                }
                p {
                    style: "margin: 0; font-size: 0.85em; opacity: 0.5;",
                    "PNG, JPG, WebP, GIF, SVG, AVIF"
                }
            }
        });
    }

    let rel = row.blob_path.clone()?;
    let rel_path = std::path::Path::new(&rel).to_path_buf();
    let data_url = crate::local_mode::images::data_url_for_blob(&vault, &rel_path)?;

    // Plans-Phase-2-editor-auto-focus: image-note tab needs to be focusable
    // so arrow keys / page-up/down can scroll the viewer when the note has
    // just been opened from the explorer. Programmatic focus only — the
    // tabindex=-1 keeps the container out of the natural tab cycle.
    let crate::editor::RequestEditorFocus(mut focus_request) = use_context();
    let note_id_for_focus = note_id.to_string();

    Some(rsx! {
        div {
            class: "operon-local-image-view",
            "data-testid": "image-note-view",
            "data-note-id": "{note_id}",
            tabindex: "-1",
            onmounted: move |evt| {
                let wants_focus = focus_request
                    .read()
                    .as_deref()
                    .map(|id| id == note_id_for_focus.as_str())
                    .unwrap_or(false);
                if wants_focus {
                    drop(evt.set_focus(true));
                    focus_request.set(None);
                }
            },
            style: "display: flex; align-items: center; justify-content: center; height: 100%; overflow: auto; padding: 1rem; background: var(--operon-bg, #111);",
            img {
                src: "{data_url}",
                alt: "{row.title}",
                style: "max-width: 100%; max-height: 100%; object-fit: contain;",
            }
        }
    })
}

/// Plans-Phase-6/5: read the active tab's textarea selectionStart via a
/// `document::eval` round-trip. Returns `None` when the textarea isn't
/// mounted or the value is out of bounds, in which case callers fall
/// back to appending at the end of the body.
///
/// Plans-Phase-9-monaco-desktop deprecated the local-mode call sites —
/// the new path splices via `MonacoChannel::splice` and Monaco computes
/// the caret server-side. Kept around because the helper is still a
/// reasonable fallback for any future textarea-shell and the unit
/// tests of `splice_at_caret` reach it transitively.
#[allow(dead_code)]
async fn read_caret_pos(tab_id: TabId) -> Option<usize> {
    let script = format!(
        "(function() {{ const t = document.querySelector('[data-tab-id=\"{}\"]'); \
         dioxus.send(t ? t.selectionStart : -1); }})();",
        tab_id.0
    );
    let mut eval = document::eval(&script);
    let n: i64 = eval.recv().await.ok()?;
    if n < 0 {
        None
    } else {
        Some(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::splice_at_caret;

    #[test]
    fn splice_empty_body() {
        assert_eq!(splice_at_caret("", 0, "hello"), "hello");
        assert_eq!(splice_at_caret("", 999, "hello"), "hello");
    }

    #[test]
    fn splice_at_zero() {
        assert_eq!(splice_at_caret("world", 0, "hello "), "hello world");
    }

    #[test]
    fn splice_in_middle() {
        assert_eq!(splice_at_caret("hello world", 5, " there,"), "hello there, world");
    }

    #[test]
    fn splice_past_end_appends() {
        assert_eq!(splice_at_caret("ab", 100, "c"), "abc");
    }

    #[test]
    fn splice_snaps_to_char_boundary() {
        // Multibyte char (é = 2 bytes); pos in the middle should snap left.
        let s = "café";
        // 'é' starts at byte 3, ends at byte 5. pos=4 is mid-char.
        let out = splice_at_caret(s, 4, "X");
        // Expect insertion at the char boundary at position 3 (before é).
        assert_eq!(out, "cafXé");
    }
}

/// Splice `insert` into `body` at character offset `pos` (snapped to the
/// nearest char boundary). Falls back to appending when `pos` is past
/// the body's end or no boundary fits.
#[allow(dead_code)]
fn splice_at_caret(body: &str, pos: usize, insert: &str) -> String {
    if body.is_empty() {
        return insert.to_string();
    }
    if pos >= body.len() {
        return format!("{body}{insert}");
    }
    let mut adj = pos;
    while adj > 0 && !body.is_char_boundary(adj) {
        adj -= 1;
    }
    let (before, after) = body.split_at(adj);
    format!("{before}{insert}{after}")
}

/// Save button rendered inside the editor toolbar. Calls the installed
/// `LocalSaveAction` on click.
#[component]
pub fn LocalSaveButton(action: LocalSaveAction, dirty: bool) -> Element {
    let label = if dirty { "Save \u{2022}" } else { "Save" };
    rsx! {
        button {
            r#type: "button",
            class: "px-2 py-1 text-xs rounded border border-[var(--operon-border)] hover:bg-[var(--operon-hover)]",
            "data-testid": "editor-save-button",
            "data-dirty": if dirty { "true" } else { "false" },
            onclick: move |_| action.callback.call(()),
            "{label}"
        }
    }
}

/// Local-Mode editor body: a plain textarea bound to `Tab.content`, or
/// an image viewer when the active note is `NoteKind::Image`.
///
/// The shared cloud `MarkdownFormatPlugin::render_edit` mounts Monaco, which
/// today only initializes on `target_arch = "wasm32"` — the desktop build
/// renders a non-functional placeholder. Local Mode bypasses that placeholder
/// with this simple textarea so notes are actually editable. Save lives off
/// `Ctrl+S` (Shell-level binding + a textarea-local fallback) and the floating
/// `LocalSaveButton` rendered by `LocalShellOverlay`. Tab title chrome comes
/// from the existing `TabStrip` above this body.
#[component]
pub fn LocalNoteEditor(tab_id: TabId, action: LocalSaveAction) -> Element {
    let tabs: Signal<TabManager> = use_context();
    // Plans-Phase-6-image-notes: image-tab view dependencies. Hooks must
    // run unconditionally; the actual rendering is gated below.
    let note_repo_for_image: crate::local_mode::desktop::LocalNoteRepo = use_context();
    let project_repo_for_image: crate::local_mode::desktop::LocalProjectRepo = use_context();
    let vault_for_image: crate::local_mode::desktop::CurrentVaultRoot = use_context();
    // Plans-Phase-5-vfs-wikilinks: link-picker visibility signal. Cmd/Ctrl+K
    // toggles it open; <LinkPicker> closes itself on pick / Escape.
    let mut link_picker_open: Signal<bool> = use_signal(|| false);
    // Plans-Phase-9-wikilink-picker (rev 1): viewport coords of an active
    // editor right-click menu, or `None`. Right-click on the textarea
    // captures `client_coordinates()` here; the menu has a single
    // "Insert reference…" item that flips `link_picker_open`.
    let mut editor_menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    // Plans-Phase-9-monaco-desktop (rev 1): channel the host writes
    // once Monaco is mounted. Picker / paste / drop / image-picker
    // splice through this so Monaco's buffer stays in sync with the
    // `Tab.content` mirror Rust holds.
    let monaco_channel: Signal<Option<MonacoChannel>> = use_signal(|| None);
    // Flips true once MonacoEditorHost receives the JS `{type:"mounted"}`
    // handshake. Splice messages sent to `monaco_channel` *before* this
    // handshake are silently dropped by the Dioxus eval bridge — same
    // pre-recv-loop dropping hazard the `setContent` path in
    // `editor_host.rs:339-348` handles. The paste handler waits on this
    // signal in addition to `monaco_channel`.
    let monaco_ready_for_paste: Signal<bool> = use_signal(|| false);

    // Plans-Phase-6-image-notes: install a JS paste listener that captures
    // clipboard image bytes and posts them back via dioxus.send. We
    // listen for messages in a use_future, write the blob, create a child
    // image-note under the active markdown note, and append ![[…]] to
    // the body.
    {
        let mut tabs_for_paste = tabs;
        let crate::local_mode::desktop::LocalNoteRepo(note_repo_for_paste) =
            note_repo_for_image.clone();
        let crate::local_mode::desktop::LocalProjectRepo(project_repo_for_paste) =
            project_repo_for_image.clone();
        let crate::local_mode::desktop::CurrentVaultRoot(vault_for_paste) =
            vault_for_image.clone();
        // The new child image-note must show up in the explorer immediately
        // after paste; bumping `LocalNoteVersion` re-fetches the project's
        // note list. The existing splice updates the markdown body, but
        // without this bump the parent's child list is stale.
        let crate::local_mode::explorer::LocalNoteVersion(mut note_version_for_paste) =
            use_context::<crate::local_mode::explorer::LocalNoteVersion>();
        use_future(move || {
            let note_repo = note_repo_for_paste.clone();
            let project_repo = project_repo_for_paste.clone();
            let vault_signal = vault_for_paste;
            async move {
                // Idempotent paste-listener install via a capture-phase
                // `keydown` on document. Capture phase runs *before*
                // Monaco's own keydown handler, so even if Monaco
                // `stopPropagation`s, our message still goes out. We do
                // NOT `preventDefault`, so Monaco's native text paste
                // still proceeds for non-image clipboards (arboard
                // returns ContentNotAvailable and the Rust loop simply
                // skips). The wired-once guard prevents accumulating
                // listeners across hot reloads / tab re-mounts.
                // The `dioxus.send` reference inside this eval string is
                // bound to THIS eval's query slot. If we kept a single
                // `.current` pointer per the wired-once pattern, any
                // earlier editor mount that has since unmounted would
                // leave a *stale* function on `.current` whose
                // `dioxus.send` throws `null is not an object (window
                // .getQuery(N).rustSend)`. Instead we keep a stack of
                // dispatchers — every mount pushes; a paste tries the
                // latest, falling through to the previous on failure.
                // Dead entries are popped lazily when their `send`
                // throws. Listeners are registered globally once.
                // The keydown filter is intentionally lax: we fire any
                // time a Monaco textarea is *mounted* in the DOM, not
                // only when it has focus. Users routinely click a note
                // in the explorer (focus → row) and then Ctrl+V without
                // first clicking back into the editor — the previous
                // `activeElement.classList.contains('inputarea')` gate
                // silently swallowed those pastes.
                //
                // The image-note empty-state has its own keydown listener
                // gated on `[data-testid="image-note-empty"]`, so on an
                // image-note tab Monaco isn't mounted and this listener
                // does nothing. Each markdown editor tab pushes its own
                // dispatcher; the broadcast goes to all live ones, and
                // the Rust handler short-circuits unless the active
                // tab id matches its own (`self_tab_id` check below).
                // Always replace the keydown handler on each install. The
                // previous design used a wired-once `__operonMarkdownPasteWired`
                // flag which persisted across hot reloads — meaning any
                // older code's listener (with old filter logic) stayed
                // attached forever and a new build's listener never
                // installed. Now we track the current handler under a
                // known name and remove it before adding the fresh one.
                let mut eval = document::eval(
                    "(function() { \
                        if (!window.__operonMarkdownPasteSenders) { \
                            window.__operonMarkdownPasteSenders = []; \
                        } \
                        window.__operonMarkdownPasteSend = function(payload) { \
                            const list = window.__operonMarkdownPasteSenders; \
                            const stillAlive = []; \
                            for (let i = 0; i < list.length; i++) { \
                                try { list[i](payload); stillAlive.push(list[i]); } \
                                catch (e) { /* dead, skip */ } \
                            } \
                            window.__operonMarkdownPasteSenders = stillAlive; \
                        }; \
                        if (window.__operonMarkdownPasteHandler) { \
                            document.removeEventListener('keydown', window.__operonMarkdownPasteHandler, true); \
                        } \
                        window.__operonMarkdownPasteHandler = function(e) { \
                            if (!((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey)) return; \
                            if (e.key !== 'v' && e.key !== 'V') return; \
                            if (!document.querySelector('textarea.inputarea')) return; \
                            console.log('operon: markdown keydown intercepted, dispatching'); \
                            window.__operonMarkdownPasteSend({ request: 'paste' }); \
                        }; \
                        document.addEventListener('keydown', window.__operonMarkdownPasteHandler, true); \
                        console.log('operon: markdown paste handler (re)installed'); \
                        window.__operonMarkdownPasteSenders.push(function(payload) { dioxus.send(payload); }); \
                        console.log('operon: markdown dispatcher pushed; stack size now', window.__operonMarkdownPasteSenders.length); \
                    })();",
                );
                let self_tab_id = tab_id;
                loop {
                    let msg: serde_json::Value = match eval.recv().await {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                    // Single source of truth: arboard reads the OS clipboard
                    // directly, sidestepping wry/WebKitGTK's broken JS
                    // clipboardData.items behavior. ContentNotAvailable is
                    // the common "user pasted text only" case — skip and
                    // let Monaco's native text paste proceed.
                    if msg.get("request").and_then(|v| v.as_str()) != Some("paste") {
                        continue;
                    }
                    // Tab-affinity gate. The keydown listener broadcasts to
                    // every live markdown editor's dispatcher; only the
                    // editor whose tab is *currently active* should act,
                    // otherwise N open markdown tabs would each create N
                    // child notes for one paste.
                    let active_tab_id = tabs_for_paste.read().active().map(|t| t.id);
                    if active_tab_id != Some(self_tab_id) {
                        continue;
                    }
                    let (bytes, mime, name): (Vec<u8>, String, String) =
                        match crate::util::clipboard::read_clipboard_image_png() {
                            Ok((b, m)) => (b, m.to_string(), "pasted".to_string()),
                            Err(e) => {
                                // Surface arboard errors to stderr so the
                                // user can see why nothing happened (e.g.
                                // missing libxcb on Linux). Text-only
                                // clipboards return `[arboard] No image
                                // on the clipboard.` here — silent skip
                                // is correct for that case.
                                if !e.contains("No image on the clipboard") {
                                    eprintln!("operon: paste-image: {e}");
                                }
                                continue;
                            }
                        };
                    // Active tab + parse uuid.
                    let active = tabs_for_paste.read().active().cloned();
                    let Some(tab) = active else { continue };
                    let Ok(parent_uuid) = Uuid::parse_str(&tab.note_id) else {
                        continue;
                    };
                    let Some(vault) = vault_signal.read().clone() else {
                        continue;
                    };
                    // Resolve project of the parent note.
                    let project_id = {
                        let projects = project_repo.list().unwrap_or_default();
                        projects.into_iter().find_map(|p| {
                            note_repo
                                .list_for_project(p.id)
                                .ok()
                                .and_then(|notes| {
                                    notes.iter().find(|n| n.id == parent_uuid).map(|_| p.id)
                                })
                        })
                    };
                    let Some(project_id) = project_id else { continue };
                    let written =
                        match crate::local_mode::images::write_image(&vault, &bytes, &mime) {
                            Ok(w) => w,
                            Err(e) => {
                                eprintln!("operon: paste-image write failed: {e}");
                                continue;
                            }
                        };
                    let stem = std::path::Path::new(&name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            format!(
                                "Pasted-{}",
                                web_time::SystemTime::now()
                                    .duration_since(web_time::UNIX_EPOCH)
                                    .map(|d| d.as_millis())
                                    .unwrap_or_default()
                            )
                        });
                    let new_note = match note_repo.create_with_kind(
                        project_id,
                        Some(parent_uuid),
                        &stem,
                        operon_store::repos::NoteKind::Image,
                    ) {
                        Ok(n) => n,
                        Err(e) => {
                            eprintln!("operon: paste-image create_with_kind failed: {e}");
                            continue;
                        }
                    };
                    let rel = written.relative_path.to_string_lossy().to_string();
                    if let Err(e) = note_repo.set_blob_path(new_note.id, Some(&rel)) {
                        eprintln!("operon: paste-image set_blob_path failed: {e}");
                    }
                    // Standard markdown image syntax. The path is
                    // vault-relative; the `MarkdownImageResolver` registered
                    // in `local_mode/desktop.rs` inflates it to a data URL
                    // at render time so wry can actually display it. We
                    // normalise to forward slashes so the same body
                    // round-trips across Linux / macOS / Windows.
                    let rel_for_md = rel.replace('\\', "/");
                    let embed = format!("![{stem}]({rel_for_md})");
                    eprintln!(
                        "operon: paste-image: created note {} with blob {}; splicing {}",
                        new_note.id, rel, embed
                    );
                    // Append the markdown image embed to `Tab.content`. The
                    // MonacoEditorHost prop-mirror at
                    // `editor_host.rs:336-349` sees the updated content
                    // prop on the next render and pushes it via the
                    // gated `setContent` path — the same path that loads
                    // initial note content (which is known-good). This
                    // bypasses `MonacoChannel.splice` entirely, avoiding
                    // accumulated stale-eval / dropped-message bugs.
                    //
                    // Trade-off: insertion lands at end-of-document, not
                    // at the cursor. Acceptable for paste-image; precise
                    // cursor insertion is a separate concern.
                    let current = tabs_for_paste
                        .read()
                        .get(tab.id)
                        .map(|t| t.content.clone())
                        .unwrap_or_default();
                    let prev_len = current.len();
                    let next = if current.is_empty() {
                        embed.clone()
                    } else if current.ends_with('\n') {
                        format!("{current}{embed}\n")
                    } else {
                        format!("{current}\n{embed}\n")
                    };
                    let next_len = next.len();
                    tabs_for_paste.write().set_content(tab.id, next);
                    tabs_for_paste.write().set_dirty(tab.id, true);
                    eprintln!(
                        "operon: paste-image: appended {embed} via Tab.content (len {prev_len} -> {next_len})"
                    );
                    // Bump note_version so the explorer sees the new child
                    // image-note immediately under the markdown parent.
                    note_version_for_paste.with_mut(|v| *v += 1);
                }
            }
        });
    }

    let snapshot = tabs.read().get(tab_id).cloned();
    let Some(tab) = snapshot else {
        return rsx! {
            div {
                class: "operon-local-editor-empty",
                "data-testid": "editor-empty",
                "No note selected."
            }
        };
    };

    // Plans-Phase-6-image-notes: image notes now flow through
    // `ImageFormatPlugin` (`plugin::registry`), so they never reach
    // `LocalNoteEditor`. The unused `_for_image` repo handles below
    // are kept until the surrounding paste / drop / picker code is
    // migrated off them in a follow-up cleanup.
    let _ = (&note_repo_for_image, &project_repo_for_image, &vault_for_image);

    let content = tab.content.clone();
    eprintln!(
        "operon: LocalNoteEditor render tab={:?} content_len={}",
        tab_id,
        content.len()
    );

    // Plans-Phase-6-image-notes: Cmd/Ctrl+Shift+I opens an image picker,
    // writes the chosen file, mints a child image-note under the current
    // note, and appends an Obsidian-style `![[…]]` reference to the
    // current body. Caret-position insertion would require a JS bridge
    // (textarea selectionStart) — acceptable follow-up.
    let mut tabs_for_image = tabs;
    let crate::local_mode::desktop::LocalNoteRepo(note_repo_for_paste) =
        note_repo_for_image.clone();
    let crate::local_mode::desktop::CurrentVaultRoot(vault_signal_for_paste) =
        vault_for_image.clone();

    // Plans-Phase-6-image-notes (drop-to-note-area): capture handles for
    // the textarea ondrop handler before the picker closure below moves
    // its own copies. An image dropped onto the editor body becomes a
    // child image-note under the active note (mirrors the explorer-row
    // drop and the clipboard paste flow) and the body gets an
    // Obsidian-style `![[…]]` embed at the caret.
    let crate::local_mode::desktop::LocalNoteRepo(note_repo_for_drop) =
        note_repo_for_image.clone();
    let crate::local_mode::desktop::LocalProjectRepo(project_repo_for_drop) =
        project_repo_for_image.clone();
    let vault_signal_for_drop = vault_signal_for_paste;
    let tabs_for_drop = tabs;

    // Plans-Phase-9-monaco-desktop (rev 1): clone before the
    // `use_callback(move ...)` consumes `tab` so we can still pass
    // note_id into MonacoEditorHost.
    let tab_note_id_for_host = tab.note_id.clone();
    let on_insert_image_via_picker = use_callback(move |_: ()| {
        let Ok(parent_uuid) = Uuid::parse_str(&tab.note_id) else {
            return;
        };
        let Some(vault) = vault_signal_for_paste.read().clone() else {
            return;
        };
        let project_id_opt = {
            let crate::local_mode::desktop::LocalProjectRepo(prepo) =
                project_repo_for_image.clone();
            let projects = prepo.list().unwrap_or_default();
            projects.iter().find_map(|p| {
                note_repo_for_paste
                    .list_for_project(p.id)
                    .ok()
                    .and_then(|notes| notes.iter().find(|n| n.id == parent_uuid).map(|_| p.id))
            })
        };
        let Some(project_id) = project_id_opt else {
            return;
        };
        let note_repo = note_repo_for_paste.clone();
        spawn(async move {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title("Insert image")
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
                    eprintln!("operon: insert image read failed: {e}");
                    return;
                }
            };
            let mime = match path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .as_deref()
            {
                Some("png") => "image/png",
                Some("jpg") | Some("jpeg") => "image/jpeg",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                Some("svg") => "image/svg+xml",
                Some("avif") => "image/avif",
                _ => return,
            };
            let written = match crate::local_mode::images::write_image(&vault, &bytes, mime) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("operon: insert image write failed: {e}");
                    return;
                }
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Image")
                .to_string();
            let new_note = match note_repo.create_with_kind(
                project_id,
                Some(parent_uuid),
                &stem,
                operon_store::repos::NoteKind::Image,
            ) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("operon: insert image create_with_kind failed: {e}");
                    return;
                }
            };
            let rel = written.relative_path.to_string_lossy().to_string();
            if let Err(e) = note_repo.set_blob_path(new_note.id, Some(&rel)) {
                eprintln!("operon: insert image set_blob_path failed: {e}");
            }
            let short = operon_store::vfs::short_id(new_note.id);
            let embed = format!("![[{}^{}]]", stem, short);
            // Plans-Phase-9-monaco-desktop (rev 1): splice via Monaco.
            if let Some(channel) = *monaco_channel.peek() {
                channel.splice(&embed);
                tabs_for_image.write().set_dirty(tab_id, true);
            } else {
                let current = tabs_for_image
                    .read()
                    .get(tab_id)
                    .map(|t| t.content.clone())
                    .unwrap_or_default();
                let next = if current.ends_with('\n') || current.is_empty() {
                    format!("{current}{embed}\n")
                } else {
                    format!("{current}\n{embed}\n")
                };
                tabs_for_image.write().set_content(tab_id, next);
                tabs_for_image.write().set_dirty(tab_id, true);
            }
        });
    });

    let mut tabs_for_link = tabs;
    let on_pick_link = move |picked: PickedLink| {
        // Plans-Phase-9-wikilink-picker (rev 1): image picks insert the
        // embed form so MarkdownView's `WikiLinkImageResolver` renders an
        // inline `<img>`; markdown / project picks stay on the clickable
        // text-anchor `[[…]]` form.
        let inserted = if picked.embed {
            format!("![[{}]]", picked.target)
        } else {
            format!("[[{}]]", picked.target)
        };
        // Plans-Phase-9-monaco-desktop (rev 1): splice through Monaco
        // (caret-aware, undo-aware). The resulting onChange propagates
        // back into `Tab.content` automatically.
        if let Some(channel) = *monaco_channel.peek() {
            channel.splice(&inserted);
            tabs_for_link.write().set_dirty(tab_id, true);
        } else {
            let current = tabs_for_link
                .read()
                .get(tab_id)
                .map(|t| t.content.clone())
                .unwrap_or_default();
            let next = if current.ends_with('\n') || current.is_empty() {
                format!("{current}{inserted}\n")
            } else {
                format!("{current}\n{inserted}\n")
            };
            tabs_for_link.write().set_content(tab_id, next);
            tabs_for_link.write().set_dirty(tab_id, true);
        }
    };

    // Plans-Phase-9-monaco-desktop (rev 6): the on_change handle Monaco
    // calls into needs to outlive `LocalNoteEditor` re-renders. The
    // recv loop inside `MonacoEditorHost` captures `on_change` once
    // when its `use_future` first runs, so a fresh `EventHandler::new`
    // built every render would leave the loop pointing at a stale
    // closure (and the user's keystrokes would never reach
    // `Tab.content`, breaking the Split / View preview). `use_callback`
    // returns a `Copy` `Callback<String>` whose underlying closure is
    // pinned for the component's lifetime — wrap it once in the prop's
    // `EventHandler::new` and the recv loop's old reference still
    // routes to the live target.
    let mut tabs_for_propagate = tabs;
    let propagate_content = use_callback(move |new_content: String| {
        tabs_for_propagate
            .write()
            .set_content(tab_id, new_content);
        tabs_for_propagate.write().set_dirty(tab_id, true);
    });

    // Plans-Phase-9-monaco-desktop (rev 1): wrap MonacoEditorHost in a
    // sizing div whose handlers catch drag/drop, right-click, and the
    // capture-phase keybindings Monaco swallows. Monaco itself is
    // mounted inside `MonacoEditorHost` via the desktop bridge.
    let on_action = {
        let mut link_picker_open = link_picker_open;
        let action = action.clone();
        let on_insert_image_via_picker_for_action = on_insert_image_via_picker;
        EventHandler::new(move |act: String| match act.as_str() {
            "save" => action.callback.call(()),
            "linkpicker" => link_picker_open.set(true),
            "imagepicker" => on_insert_image_via_picker_for_action.call(()),
            _ => {}
        })
    };
    rsx! {
        div {
            class: "operon-local-editor-host",
            "data-testid": "local-note-editor-host",
            "data-tab-id": "{tab_id.0}",
            // Plans-Phase-9-monaco-desktop (rev 11): absolute-inset
            // against the positioned ancestor (`.operon-main-body` /
            // `.operon-local-split-edit`). Children (MonacoEditorHost
            // wrapping div, optional ContextMenu, LinkPicker) are all
            // either absolute-positioned or fixed — no flex needed
            // here. Removing `display: flex; flex-direction: column;`
            // simplifies the layout chain and rules out a flex-with-
            // no-in-flow-children edge case as the cause of the
            // 0×0 host the user saw in Split mode.
            style: "position: absolute; inset: 0;",
            // Plans-Phase-6-image-notes (drop-to-note-area): preventing
            // default on `ondragover` is what tells the browser this
            // element is a valid drop target — without it, `ondrop`
            // never fires.
            ondragover: move |evt| evt.prevent_default(),
            ondrop: {
                let note_repo_outer = note_repo_for_drop.clone();
                let project_repo_outer = project_repo_for_drop.clone();
                move |evt: Event<DragData>| {
                    evt.prevent_default();
                    let files = evt.data().files();
                    if files.is_empty() {
                        return;
                    }
                    for f in files {
                        let name = f.name();
                        let lower = name.to_ascii_lowercase();
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
                            continue;
                        };
                        let note_repo = note_repo_outer.clone();
                        let project_repo = project_repo_outer.clone();
                        let vault_signal = vault_signal_for_drop;
                        let mut tabs_sig = tabs_for_drop;
                        let cur_tab_id = tab_id;
                        spawn(async move {
                            let bytes = match f.read_bytes().await {
                                Ok(b) => b.to_vec(),
                                Err(e) => {
                                    eprintln!(
                                        "operon: drop-image read_bytes failed: {e:?}"
                                    );
                                    return;
                                }
                            };
                            let active = tabs_sig.read().get(cur_tab_id).cloned();
                            let Some(tab) = active else {
                                return;
                            };
                            let Ok(parent_uuid) = Uuid::parse_str(&tab.note_id) else {
                                return;
                            };
                            let Some(vault) = vault_signal.read().clone() else {
                                return;
                            };
                            let project_id_opt = {
                                let projects = project_repo.list().unwrap_or_default();
                                projects.into_iter().find_map(|p| {
                                    note_repo
                                        .list_for_project(p.id)
                                        .ok()
                                        .and_then(|notes| {
                                            notes
                                                .iter()
                                                .find(|n| n.id == parent_uuid)
                                                .map(|_| p.id)
                                        })
                                })
                            };
                            let Some(project_id) = project_id_opt else {
                                return;
                            };
                            let written = match crate::local_mode::images::write_image(
                                &vault, &bytes, mime,
                            ) {
                                Ok(w) => w,
                                Err(e) => {
                                    eprintln!("operon: drop-image write failed: {e}");
                                    return;
                                }
                            };
                            let stem = std::path::Path::new(&name)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Image")
                                .to_string();
                            let new_note = match note_repo.create_with_kind(
                                project_id,
                                Some(parent_uuid),
                                &stem,
                                operon_store::repos::NoteKind::Image,
                            ) {
                                Ok(n) => n,
                                Err(e) => {
                                    eprintln!(
                                        "operon: drop-image create_with_kind failed: {e}"
                                    );
                                    return;
                                }
                            };
                            let rel = written.relative_path.to_string_lossy().to_string();
                            if let Err(e) = note_repo.set_blob_path(new_note.id, Some(&rel))
                            {
                                eprintln!(
                                    "operon: drop-image set_blob_path failed: {e}"
                                );
                            }
                            let short = operon_store::vfs::short_id(new_note.id);
                            let embed = format!("![[{stem}^{short}]]");
                            // Plans-Phase-9-monaco-desktop (rev 1):
                            // splice via Monaco (caret-aware, undo-aware).
                            if let Some(channel) = *monaco_channel.peek() {
                                channel.splice(&embed);
                                tabs_sig.write().set_dirty(cur_tab_id, true);
                            } else {
                                let current = tabs_sig
                                    .read()
                                    .get(cur_tab_id)
                                    .map(|t| t.content.clone())
                                    .unwrap_or_default();
                                let next = if current.ends_with('\n') || current.is_empty() {
                                    format!("{current}{embed}\n")
                                } else {
                                    format!("{current}\n{embed}\n")
                                };
                                tabs_sig.write().set_content(cur_tab_id, next);
                                tabs_sig.write().set_dirty(cur_tab_id, true);
                            }
                        });
                    }
                }
            },
            // Plans-Phase-9-wikilink-picker (rev 1): right-click reveals a
            // tiny menu with one item that opens the LinkPicker. The
            // bootstrap script's window-level keydown listener handles
            // Cmd/Ctrl+K too; this is the discoverable mouse path.
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                let coords = evt.client_coordinates();
                editor_menu_pos.set(Some((coords.x as i32, coords.y as i32)));
            },
            MonacoEditorHost {
                note_id: tab_note_id_for_host.clone(),
                content: content.clone(),
                language: LanguageDescriptor::markdown(),
                // Plans-Phase-9-monaco-desktop (rev 6): forward to the
                // stable `Callback` declared above so the propagation
                // chain survives Monaco's first-render capture.
                on_change: EventHandler::new(move |new_content: String| {
                    propagate_content.call(new_content);
                }),
                channel_sink: monaco_channel,
                ready_sink: Some(monaco_ready_for_paste),
                on_action: on_action,
            }
        }
        if let Some((x, y)) = *editor_menu_pos.read() {
            crate::local_mode::ui::ContextMenu {
                x: x,
                y: y,
                items: vec![crate::local_mode::ui::ContextMenuItem::new(
                    "Insert reference\u{2026}",
                    Callback::new(move |_| {
                        editor_menu_pos.set(None);
                        link_picker_open.set(true);
                    }),
                )],
                on_dismiss: Callback::new(move |_| editor_menu_pos.set(None)),
            }
        }
        if *link_picker_open.read() {
            LinkPicker {
                open: link_picker_open,
                on_pick: on_pick_link,
            }
        }
    }
}
