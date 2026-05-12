//! A single project row inside [`crate::local_mode::explorer::ExplorerPanel`].

use dioxus::prelude::*;
use dioxus::html::HasFileData;
use operon_store::repos::{LocalNoteRepository, LocalProject};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

use operon_store::repos::NoteKind;

use keyboard_types::Modifiers;

use crate::local_mode::desktop::LocalNoteRepo;
use crate::local_mode::explorer::creatable_kind::{build_creatable_menu, CreatableKind};
use crate::local_mode::explorer::{
    ExplorerUndoCtx, FocusedNode, LastClicked, LocalNoteVersion, MultiSelected, NodeKey,
    NotesByProjectCtx, VisibleFlat,
};
use crate::local_mode::ui::{
    classify_drop_position, ContextMenu, ContextMenuItem, DragKind, DragSession, DropPosition,
    InlineRename,
};
use crate::persistence::Persistence;

#[derive(Props, Clone, PartialEq)]
pub struct ProjectRowProps {
    pub project: LocalProject,
    pub is_open: bool,
    pub selected: bool,
    /// True when one of this project's notes is open in the active tab.
    /// Mirrors the NoteRow accent so the user can scan to the active branch.
    pub tab_active: bool,
    pub in_rename: bool,
    /// Source row of the current Cut clipboard.
    pub cut: bool,
    /// Whether the clipboard currently holds a note payload (enables Paste).
    pub has_clip_note: bool,
    /// Whether some drag is in progress (used to filter drop visuals).
    pub drag_active: bool,
    pub on_select: Callback<Uuid>,
    pub on_rename: Callback<(Uuid, String)>,
    pub on_delete: Callback<Uuid>,
    pub on_request_rename: Callback<Uuid>,
    pub on_request_delete: Callback<Uuid>,
    pub on_toggle: Callback<Uuid>,
    /// Plans-Phase-1-note-creation-context-menu: project-level Add note is
    /// now kind-aware. The submenu's Markdown leaf goes through the existing
    /// `note_repo.create` path (auto-rename triggered on the new row); the
    /// Image leaf opens the same native file picker that the old standalone
    /// `Add image note…` item used. The "Artifact ▶" submenu's children
    /// dispatch with `CreatableKind::Artifact(...)` so the handler can
    /// inject the matching scaffold body.
    pub on_add_note: Callback<(Uuid, CreatableKind)>,
    /// Plans-Phase-6-image-notes: external image-file drops onto this
    /// project row land as top-level image-notes in the project. Tuple is
    /// (project_id, bytes, suggested filename).
    pub on_drop_image_file: Callback<(Uuid, Vec<u8>, String)>,
    pub on_cut: Callback<Uuid>,
    pub on_copy: Callback<Uuid>,
    pub on_paste: Callback<Uuid>,
    pub on_drop_project_on_project: Callback<(Uuid, Uuid, DropPosition)>,
    pub on_drop_note_on_project: Callback<(Uuid, Uuid, DropPosition)>,
    /// Plans-Phase-4-multiselect-aria: bulk variants invoked from the
    /// context menu when the right-clicked project is itself in
    /// `MultiSelected` (size >= 2). Cut/Copy populate the `BulkClipboard`;
    /// Delete raises the existing `pending_bulk_delete` confirmation flag.
    pub on_bulk_cut: Callback<()>,
    pub on_bulk_copy: Callback<()>,
    pub on_bulk_request_delete: Callback<()>,
    /// M1-companion-claude-code: bind / clear the project's git repository
    /// path. The companion-pane Claude session runs with cwd=repo_path.
    /// Tuple is (project_id, new_path | None).
    pub on_set_repo_path: Callback<(Uuid, Option<PathBuf>)>,
}

#[component]
pub fn ProjectRow(props: ProjectRowProps) -> Element {
    let menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut menu_pos_setter = menu_pos;
    // Operon-Phase-3-note-kind-dropdown: the + button opens a dropdown of
    // every NoteKind in `NoteKind::all_creatable()` instead of hard-coding
    // Markdown. Tracked by its own signal so it does not collide with the
    // right-click context menu on the same row.
    let add_menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut add_menu_pos_setter = add_menu_pos;
    // Plans-Phase-7-projectrow-forbidden: tri-state indicator mirroring
    // NoteRow. Some(Ok(pos)) → allowed; Some(Err(())) → forbidden
    // (self-drop, or a position not allowed for the dragged kind);
    // None → no drag over this row.
    let drop_indicator: Signal<Option<Result<DropPosition, ()>>> = use_signal(|| None);
    let mut drop_indicator_setter = drop_indicator;
    let DragSession(mut drag_session) = use_context();
    // Plans-Phase-8-explorer-undo: panel-scope undo handle for the
    // "Undo last action" menu entry.
    let ExplorerUndoCtx { history, on_undo, on_redo } = use_context::<ExplorerUndoCtx>();
    // Plans-Phase-4-multiselect-aria: project rows now participate in
    // the same multi-select set as note rows (Cmd/Ctrl+click toggles,
    // Shift+click range-fills via VisibleFlat).
    let MultiSelected(mut multi_selected) = use_context();
    let LastClicked(mut last_clicked) = use_context();
    let VisibleFlat(visible_flat) = use_context();
    let FocusedNode(mut focused_node) = use_context();
    let NotesByProjectCtx(notes_by_project_ctx) = use_context();

    let project = props.project.clone();
    let id = project.id;
    let id_str = id.to_string();
    let name = project.name.clone();
    let repo_subtitle = project
        .repo_path
        .as_ref()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));

    let in_multi = multi_selected.read().contains(&NodeKey::Project(id));
    let bulk_count = multi_selected.read().len();
    let is_bulk = in_multi && bulk_count >= 2;
    let selected = props.selected || in_multi;
    let in_rename = props.in_rename;
    let is_open = props.is_open;
    let cut = props.cut;
    let has_clip_note = props.has_clip_note;
    let drag_active = props.drag_active;

    let on_select = props.on_select;
    let on_rename = props.on_rename;
    let _on_delete = props.on_delete;
    let on_request_rename = props.on_request_rename;
    let on_request_delete = props.on_request_delete;
    let on_toggle = props.on_toggle;
    let on_add_note = props.on_add_note;
    let on_drop_image_file = props.on_drop_image_file;
    let on_cut = props.on_cut;
    let on_copy = props.on_copy;
    let on_paste = props.on_paste;
    let on_drop_project_on_project = props.on_drop_project_on_project;
    let on_drop_note_on_project = props.on_drop_note_on_project;
    let on_bulk_cut = props.on_bulk_cut;
    let on_bulk_copy = props.on_bulk_copy;
    let on_bulk_request_delete = props.on_bulk_request_delete;
    let on_set_repo_path = props.on_set_repo_path;
    let has_repo = project.repo_path.is_some();

    let mut row_class = if selected {
        String::from("notes-explorer-row notes-explorer-row-project notes-explorer-row-active group")
    } else {
        String::from("notes-explorer-row notes-explorer-row-project group")
    };
    if props.tab_active {
        row_class.push_str(" notes-explorer-row-tab-active notes-explorer-row-tab-active-project");
    }
    if cut {
        row_class.push_str(" notes-explorer-row-cut");
    }
    let style = "--depth: 0;";

    let initial_name = name.clone();
    let dismiss_menu = use_callback(move |_: ()| menu_pos_setter.set(None));
    let dismiss_add_menu = use_callback(move |_: ()| add_menu_pos_setter.set(None));

    // Dropdown items built from the shared layout in `creatable_kind.rs`
    // — top-level plain kinds plus the typed "Artifact ▶" submenu —
    // so this project-level "+" stays in lockstep with the per-note "+"
    // dropdown and the right-click "Add child / sibling" submenus.
    let add_menu_items: Vec<ContextMenuItem> =
        build_creatable_menu(Callback::new(move |kind| {
            on_add_note.call((id, kind));
        }));

    let mut paste_item = ContextMenuItem::new(
        "Paste",
        Callback::new(move |_| {
            on_paste.call(id);
        }),
    );
    paste_item.enabled = has_clip_note;

    // Bulk-import a folder of `.md` files as `NoteKind::Skill` notes
    // under this project. Title = file stem; body = full file content
    // (including frontmatter). Idempotent: existing Skill notes with
    // the same title are skipped, so re-running the import after a
    // skill body edit only adds new files. README.md is filtered.
    let LocalNoteRepo(note_repo_for_import) = use_context();
    let persistence_for_import: Arc<dyn Persistence> = use_context();
    let LocalNoteVersion(note_version_for_import) = use_context();
    let import_skills = {
        let note_repo = note_repo_for_import.clone();
        let persistence = persistence_for_import.clone();
        let mut version_setter = note_version_for_import;
        Callback::new(move |_| {
            let note_repo = note_repo.clone();
            let persistence = persistence.clone();
            let project_id = id;
            spawn(async move {
                let folder = rfd::AsyncFileDialog::new()
                    .set_title("Pick a folder of .md skill files to import")
                    .pick_folder()
                    .await;
                let Some(handle) = folder else { return };
                let path = handle.path().to_path_buf();
                let n = import_skills_from_folder(
                    &note_repo,
                    &persistence,
                    project_id,
                    &path,
                )
                .await;
                tracing::info!(
                    target: "operon::explorer",
                    "imported {n} skill(s) from {}",
                    path.display()
                );
                version_setter.with_mut(|v| *v = v.saturating_add(1));
            });
        })
    };

    // M1-companion-claude-code: open the OS folder picker, then route the
    // selection (or a None on Clear) through `on_set_repo_path`.
    let pick_repo = {
        let on_set_repo_path = on_set_repo_path;
        Callback::new(move |_| {
            spawn(async move {
                let folder = rfd::AsyncFileDialog::new()
                    .set_title("Select repository folder for this project")
                    .pick_folder()
                    .await;
                if let Some(handle) = folder {
                    on_set_repo_path.call((id, Some(handle.path().to_path_buf())));
                }
            });
        })
    };
    let clear_repo = {
        let on_set_repo_path = on_set_repo_path;
        Callback::new(move |_| {
            on_set_repo_path.call((id, None));
        })
    };

    let menu_items: Vec<ContextMenuItem> = vec![
        ContextMenuItem::new(
            "Rename",
            Callback::new(move |_| {
                on_request_rename.call(id);
            }),
        ),
        ContextMenuItem::new(
            if has_repo {
                "Change repository\u{2026}"
            } else {
                "Set repository\u{2026}"
            },
            pick_repo,
        ),
        {
            let mut item = ContextMenuItem::new("Clear repository", clear_repo);
            item.enabled = has_repo;
            item
        },
        ContextMenuItem::submenu(
            "Add note",
            build_creatable_menu(Callback::new(move |kind| {
                on_add_note.call((id, kind));
            })),
        ),
        ContextMenuItem::new("Import skills\u{2026}", import_skills),
        if is_bulk {
            ContextMenuItem::new(
                format!("Cut {bulk_count} items"),
                Callback::new(move |_| {
                    on_bulk_cut.call(());
                }),
            )
        } else {
            ContextMenuItem::new(
                "Cut",
                Callback::new(move |_| {
                    on_cut.call(id);
                }),
            )
        },
        if is_bulk {
            ContextMenuItem::new(
                format!("Copy {bulk_count} items"),
                Callback::new(move |_| {
                    on_bulk_copy.call(());
                }),
            )
        } else {
            ContextMenuItem::new(
                "Copy",
                Callback::new(move |_| {
                    on_copy.call(id);
                }),
            )
        },
        paste_item,
        // Plans-Phase-8-explorer-undo: surface the keybinding (Cmd+Z) for
        // discovery. Disabled when the stack is empty.
        {
            let mut item = ContextMenuItem::new(
                "Undo last action",
                Callback::new(move |_| {
                    on_undo.call(());
                }),
            );
            item.enabled = !history.read().is_empty();
            item
        },
        {
            // Plans-Phase-11: paired Redo entry (Cmd/Ctrl+Shift+Z). Disabled
            // when the redo deque is empty.
            let mut item = ContextMenuItem::new(
                "Redo last action",
                Callback::new(move |_| {
                    on_redo.call(());
                }),
            );
            item.enabled = !history.read().redo_is_empty();
            item
        },
        if is_bulk {
            ContextMenuItem::new(
                format!("Delete {bulk_count} items"),
                Callback::new(move |_| {
                    on_bulk_request_delete.call(());
                }),
            )
        } else {
            ContextMenuItem::new(
                "Delete",
                Callback::new(move |_| {
                    on_request_delete.call(id);
                }),
            )
        },
    ];

    let caret_glyph = if is_open { "\u{25BE}" } else { "\u{25B8}" };

    let drop_pos_now = *drop_indicator.read();

    rsx! {
        div {
            class: "{row_class}",
            style: "{style}",
            "data-testid": "project-row",
            "data-explorer": "true",
            "data-project-id": "{id_str}",
            "data-selected": if selected { "true" } else { "false" },
            "data-open": if is_open { "true" } else { "false" },
            "data-cut": if cut { "true" } else { "false" },
            // Plans-Phase-4-multiselect-aria: WAI-ARIA tree pattern. Projects
            // are level 1; notes inside are level 2+.
            role: "treeitem",
            "aria-level": "1",
            "aria-selected": if selected { "true" } else { "false" },
            "aria-expanded": if is_open { "true" } else { "false" },
            tabindex: "0",
            draggable: "true",
            onkeydown: move |evt| {
                let key = evt.key().to_string();
                let mods = evt.modifiers();
                let with_meta = mods.contains(Modifiers::META)
                    || mods.contains(Modifiers::CONTROL);
                if with_meta && mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && key.eq_ignore_ascii_case("c")
                {
                    evt.prevent_default();
                    evt.stop_propagation();
                    crate::util::clipboard::copy_text(&id.to_string());
                    return;
                }
                if key == "ArrowDown" && mods.contains(Modifiers::SHIFT) {
                    evt.prevent_default();
                    evt.stop_propagation();
                    crate::local_mode::explorer::extend_keyboard_selection(
                        NodeKey::Project(id),
                        1,
                        &mut multi_selected,
                        &last_clicked,
                        &visible_flat,
                    );
                    if let Some(next) = super::note_row::next_visible(
                        NodeKey::Project(id),
                        1,
                        &visible_flat,
                    ) {
                        focused_node.set(Some(next));
                        super::note_row::focus_explorer_node_deferred(next);
                    }
                } else if key == "ArrowUp" && mods.contains(Modifiers::SHIFT) {
                    evt.prevent_default();
                    evt.stop_propagation();
                    crate::local_mode::explorer::extend_keyboard_selection(
                        NodeKey::Project(id),
                        -1,
                        &mut multi_selected,
                        &last_clicked,
                        &visible_flat,
                    );
                    if let Some(next) = super::note_row::next_visible(
                        NodeKey::Project(id),
                        -1,
                        &visible_flat,
                    ) {
                        focused_node.set(Some(next));
                        super::note_row::focus_explorer_node_deferred(next);
                    }
                } else if key == "ArrowDown" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    if let Some(next) = super::note_row::next_visible(
                        NodeKey::Project(id),
                        1,
                        &visible_flat,
                    ) {
                        focused_node.set(Some(next));
                        super::note_row::focus_explorer_node_deferred(next);
                    }
                } else if key == "ArrowUp" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    if let Some(next) = super::note_row::next_visible(
                        NodeKey::Project(id),
                        -1,
                        &visible_flat,
                    ) {
                        focused_node.set(Some(next));
                        super::note_row::focus_explorer_node_deferred(next);
                    }
                } else if key == "ArrowRight" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    if !is_open {
                        on_toggle.call(id);
                    } else if let Some(next) = super::note_row::next_visible(
                        NodeKey::Project(id),
                        1,
                        &visible_flat,
                    ) {
                        focused_node.set(Some(next));
                        super::note_row::focus_explorer_node_deferred(next);
                    }
                } else if key == "ArrowLeft" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    if is_open {
                        on_toggle.call(id);
                    }
                } else if key == "Home" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    let flat = visible_flat.peek().clone();
                    if let Some(first) = flat.first().copied() {
                        focused_node.set(Some(first));
                        super::note_row::focus_explorer_node_deferred(first);
                    }
                } else if key == "End" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    let flat = visible_flat.peek().clone();
                    if let Some(last) = flat.last().copied() {
                        focused_node.set(Some(last));
                        super::note_row::focus_explorer_node_deferred(last);
                    }
                } else if key == "Enter" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    on_select.call(id);
                } else if key == " " {
                    evt.prevent_default();
                    evt.stop_propagation();
                    on_toggle.call(id);
                } else if key == "F2" {
                    evt.prevent_default();
                    evt.stop_propagation();
                    on_request_rename.call(id);
                } else if (key == "Delete" || key == "Backspace")
                    && multi_selected.read().len() < 2
                {
                    evt.prevent_default();
                    evt.stop_propagation();
                    on_request_delete.call(id);
                }
            },
            onclick: move |evt| {
                evt.stop_propagation();
                let mods = evt.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                let key = NodeKey::Project(id);
                if with_meta && !mods.contains(Modifiers::SHIFT) {
                    // Plans-Phase-4-multiselect-aria: Cmd/Ctrl+click toggles
                    // this project in the multi-set without disturbing the
                    // single-select signal.
                    multi_selected.with_mut(|set| {
                        if !set.remove(&key) {
                            set.insert(key);
                        }
                    });
                    last_clicked.set(Some(key));
                    return;
                }
                if mods.contains(Modifiers::SHIFT) {
                    let mut set: std::collections::BTreeSet<NodeKey> =
                        multi_selected.read().clone();
                    let flat = visible_flat.read().clone();
                    let prev_opt = *last_clicked.read();
                    if let Some(prev) = prev_opt {
                        let a = flat.iter().position(|k| k == &prev);
                        let b = flat.iter().position(|k| k == &key);
                        if let (Some(a), Some(b)) = (a, b) {
                            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                            for k in &flat[lo..=hi] {
                                set.insert(*k);
                            }
                        } else {
                            set.insert(prev);
                            set.insert(key);
                        }
                    } else {
                        set.insert(key);
                    }
                    multi_selected.set(set);
                    return;
                }
                if !multi_selected.read().is_empty() {
                    multi_selected.set(std::collections::BTreeSet::new());
                }
                last_clicked.set(Some(key));
                on_select.call(id);
                focused_node.set(Some(NodeKey::Project(id)));
                super::note_row::focus_explorer_node_deferred(NodeKey::Project(id));
            },
            ondoubleclick: move |evt| {
                evt.stop_propagation();
                on_toggle.call(id);
            },
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                let coords = evt.client_coordinates();
                menu_pos_setter.set(Some((coords.x as i32, coords.y as i32)));
            },
            ondragstart: move |_| {
                drag_session.set(Some(DragKind::Project(id)));
            },
            ondragend: move |_| {
                drag_session.set(None);
                drop_indicator_setter.set(None);
            },
            ondragover: move |evt| {
                evt.prevent_default();
                let kind = *drag_session.read();
                let coords = evt.element_coordinates();
                // Element coordinates are relative to the row's top-left corner.
                // Estimate row height from py-1 + line height — fall back to 28px.
                let pos = classify_drop_position(coords.y, 28.0);
                // Plans-Phase-7-projectrow-forbidden: classify the drop:
                //   - allowed → Some(Ok(pos))
                //   - forbidden (self-drop or wrong position) → Some(Err(()))
                //   - no relevant drag → None
                let multi_snap = multi_selected.read().clone();
                let multi_active = multi_snap.len() >= 2;
                // Plans-Phase-4-multiselect-aria: when the drag source is in
                // a 2+ multi-set, every member must share the same
                // SiblingGroup. A note+project mix yields Err.
                let sibling_violation = if multi_active {
                    let notes_snap = notes_by_project_ctx.read();
                    !crate::local_mode::explorer::all_siblings(&multi_snap, &notes_snap)
                } else {
                    false
                };
                let next: Option<Result<DropPosition, ()>> = match kind {
                    Some(DragKind::Project(src)) => {
                        if src == id {
                            Some(Err(()))
                        } else if matches!(pos, DropPosition::Into) {
                            Some(Err(()))
                        } else if multi_snap.contains(&NodeKey::Project(src)) && sibling_violation {
                            Some(Err(()))
                        } else {
                            Some(Ok(pos))
                        }
                    }
                    Some(DragKind::Note(src)) => {
                        if !matches!(pos, DropPosition::Into) {
                            Some(Err(()))
                        } else if multi_snap.contains(&NodeKey::Note(src)) && sibling_violation {
                            Some(Err(()))
                        } else {
                            Some(Ok(pos))
                        }
                    }
                    None => None,
                };
                drop_indicator_setter.set(next);
            },
            ondragleave: move |_| {
                drop_indicator_setter.set(None);
            },
            ondrop: move |evt| {
                evt.prevent_default();
                // In-app drag wins over `evt.data().files()` — see the
                // matching comment in `note_row.rs::ondrop`. Real OS file
                // drops never go through any in-app `ondragstart`, so
                // `drag_session` is `None` and the file-drop branch runs.
                let kind = *drag_session.read();
                if kind.is_none() {
                    let files = evt.data().files();
                    if !files.is_empty() {
                        for f in files {
                            let name = f.name();
                            let lower = name.to_ascii_lowercase();
                            if !lower.ends_with(".png")
                                && !lower.ends_with(".jpg")
                                && !lower.ends_with(".jpeg")
                                && !lower.ends_with(".webp")
                                && !lower.ends_with(".gif")
                                && !lower.ends_with(".svg")
                                && !lower.ends_with(".avif")
                            {
                                continue;
                            }
                            let cb = on_drop_image_file;
                            spawn(async move {
                                if let Ok(bytes) = f.read_bytes().await {
                                    cb.call((id, bytes.to_vec(), name));
                                }
                            });
                        }
                        drop_indicator_setter.set(None);
                        return;
                    }
                }
                let coords = evt.element_coordinates();
                let pos = classify_drop_position(coords.y, 28.0);
                // Plans-Phase-4-multiselect-aria: when the source is in a
                // 2+ multi-set, gate the drop on `all_siblings` and (for
                // projects) iterate the set so every selected project
                // reorders together. The sibling guard already raised a
                // forbidden indicator during dragover; the drop just
                // mirrors that decision.
                let multi_snap = multi_selected.read().clone();
                let multi_active = multi_snap.len() >= 2;
                let siblings_ok = if multi_active {
                    let notes_snap = notes_by_project_ctx.read();
                    crate::local_mode::explorer::all_siblings(&multi_snap, &notes_snap)
                } else {
                    true
                };
                match kind {
                    Some(DragKind::Project(src))
                        if src != id && !matches!(pos, DropPosition::Into) =>
                    {
                        if multi_active && multi_snap.contains(&NodeKey::Project(src)) {
                            if siblings_ok {
                                for k in multi_snap.iter() {
                                    if let NodeKey::Project(p_id) = k {
                                        if *p_id != id {
                                            on_drop_project_on_project.call((*p_id, id, pos));
                                        }
                                    }
                                }
                            }
                        } else {
                            on_drop_project_on_project.call((src, id, pos));
                        }
                    }
                    Some(DragKind::Note(src)) if matches!(pos, DropPosition::Into) => {
                        // Note-into-project drops are still single-target
                        // by design (the panel-side handler doesn't support
                        // bulk-into-project moves). Reject if the source is
                        // part of a non-sibling multi-set.
                        if multi_active
                            && multi_snap.contains(&NodeKey::Note(src))
                            && !siblings_ok
                        {
                            // forbidden — no-op
                        } else {
                            on_drop_note_on_project.call((src, id, pos));
                        }
                    }
                    _ => {}
                }
                drag_session.set(None);
                drop_indicator_setter.set(None);
            },
            // Drag handle (the row itself is draggable, but we expose a hook).
            span {
                class: "inline-flex w-3 shrink-0 select-none text-xs opacity-70",
                "data-testid": "drag-handle",
                onclick: move |evt| {
                    evt.stop_propagation();
                    on_toggle.call(id);
                },
                "{caret_glyph}"
            }
            if cut {
                span {
                    class: "sr-only",
                    "data-testid": "clipboard-indicator",
                    "Cut"
                }
            }
            if in_rename {
                InlineRename {
                    initial: initial_name.clone(),
                    on_commit: Callback::new(move |new_name: String| {
                        on_rename.call((id, new_name));
                    }),
                    on_cancel: Callback::new(move |_| {
                        on_rename.call((id, String::new()));
                    }),
                }
            } else {
                span {
                    class: "truncate flex-1 flex items-baseline gap-2 min-w-0",
                    "data-testid": "project-row-name",
                    span { class: "truncate", "{name}" }
                    if let Some(sub) = repo_subtitle.clone() {
                        span {
                            class: "text-xs opacity-60 truncate font-mono",
                            "data-testid": "project-row-repo-subtitle",
                            title: project.repo_path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                            "{sub}"
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "opacity-0 group-hover:opacity-100 inline-flex items-center justify-center w-5 h-5 rounded text-xs hover:bg-[var(--operon-border)]",
                    "data-testid": "add-note-button",
                    "data-project-id": "{id_str}",
                    "aria-label": "Add note",
                    "aria-haspopup": "menu",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        let coords = evt.client_coordinates();
                        add_menu_pos_setter.set(Some((coords.x as i32, coords.y as i32)));
                    },
                    "+"
                }
            }
            if drag_active {
                match drop_pos_now {
                    Some(Ok(p)) => rsx! { DropIndicator { position: p } },
                    Some(Err(())) => rsx! { ForbiddenIndicator {} },
                    None => rsx! {},
                }
            }
        }
        if let Some((x, y)) = *menu_pos.read() {
            ContextMenu {
                x: x,
                y: y,
                items: menu_items,
                on_dismiss: dismiss_menu,
            }
        }
        if let Some((x, y)) = *add_menu_pos.read() {
            ContextMenu {
                x: x,
                y: y,
                items: add_menu_items,
                on_dismiss: dismiss_add_menu,
            }
        }
    }
}

#[component]
fn DropIndicator(position: DropPosition) -> Element {
    let (testid, class) = match position {
        DropPosition::Before => (
            "drop-indicator-before",
            "absolute left-0 right-0 top-0 h-0.5 bg-[var(--operon-accent)]",
        ),
        DropPosition::Into => (
            "drop-indicator-into",
            "absolute inset-0 ring-2 ring-[var(--operon-accent)] pointer-events-none",
        ),
        DropPosition::After => (
            "drop-indicator-after",
            "absolute left-0 right-0 bottom-0 h-0.5 bg-[var(--operon-accent)]",
        ),
    };
    rsx! {
        span {
            class: "{class}",
            "data-testid": "{testid}",
        }
    }
}

/// Plans-Phase-7-projectrow-forbidden: shown when a drop on this project
/// row would not be valid (project drop on self, project drop into
/// another project's body, or note drop in a Before/After zone of a
/// project). Same red-ring + no-drop-cursor as the NoteRow variant.
#[component]
fn ForbiddenIndicator() -> Element {
    rsx! {
        span {
            class: "absolute inset-0 ring-2 ring-rose-500 pointer-events-none",
            style: "cursor: no-drop;",
            "data-testid": "drop-indicator-forbidden",
        }
    }
}

/// Walk `folder` for top-level `.md` files and create a `NoteKind::Skill`
/// note under a project-root `SKILLS` index note for each one. Title =
/// file stem; body = full file contents. Idempotent: skills with a
/// matching title already present in the project are left alone, so
/// the user can re-run the import after pulling new skill files. The
/// `SKILLS` parent note is found-or-created on each run; its body is
/// regenerated from the folder's `README.md` (when present) plus an
/// auto-generated list of every skill child as clickable
/// `operon://note/<uuid>` links. Returns the number of skill rows
/// actually created.
async fn import_skills_from_folder(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    folder: &Path,
) -> usize {
    let entries = match std::fs::read_dir(folder) {
        Ok(it) => it,
        Err(e) => {
            tracing::warn!(
                target: "operon::explorer",
                "import_skills: read_dir({}) failed: {e}",
                folder.display()
            );
            return 0;
        }
    };

    // Find-or-create the SKILLS container so every imported skill
    // becomes a child of one index note instead of cluttering the
    // project root. Re-imports reuse the existing container.
    let skills_parent_id =
        match find_or_create_skills_parent(note_repo, persistence, project_id).await {
            Some(id) => id,
            None => {
                tracing::warn!(
                    target: "operon::explorer",
                    "import_skills: failed to find or create SKILLS parent note"
                );
                return 0;
            }
        };

    let existing_titles: HashSet<String> = note_repo
        .list_for_project(project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| matches!(n.kind, NoteKind::Skill))
        .map(|n| n.title)
        .collect();

    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if stem.eq_ignore_ascii_case("readme") {
            continue;
        }
        if existing_titles.contains(&stem) {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "operon: import_skills read failed for {}: {e}",
                    path.display()
                );
                continue;
            }
        };
        match note_repo.create_with_kind(
            project_id,
            Some(skills_parent_id),
            &stem,
            NoteKind::Skill,
        ) {
            Ok(row) => {
                if let Err(e) = persistence
                    .save(&row.id.to_string(), body.as_bytes())
                    .await
                {
                    eprintln!("operon: import_skills save failed for {stem}: {e}");
                    continue;
                }
                count += 1;
            }
            Err(e) => {
                eprintln!("operon: import_skills create_with_kind failed for {stem}: {e}");
            }
        }
    }

    // Regenerate the SKILLS index body. If the source folder ships a
    // `README.md`, it becomes the prose preamble (the user's authored
    // documentation about the chain). The auto-generated section
    // below it lists every current skill child as an
    // `operon://note/<uuid>` link so the renderer surfaces clickable
    // navigation. Re-imports overwrite the body — manual edits to
    // SKILLS are not preserved across re-imports; that's the
    // tradeoff for keeping the index honest.
    let readme = read_folder_readme(folder);
    let body = build_skills_index_body(note_repo, project_id, skills_parent_id, readme.as_deref());
    if let Err(e) = persistence
        .save(&skills_parent_id.to_string(), body.as_bytes())
        .await
    {
        tracing::warn!(
            target: "operon::explorer",
            "import_skills: SKILLS index save failed: {e}"
        );
    }

    count
}

/// Title used for the auto-managed skill index note. Stored at project
/// root with `NoteKind::Markdown`. Re-imports look up by exact title +
/// kind + root-level position so renamed / moved indexes get a new one
/// rather than colliding.
const SKILLS_PARENT_TITLE: &str = "SKILLS";

/// Find the project's `SKILLS` index note (root-level Markdown note
/// titled `SKILLS`), or create one if absent. Returns its id, or
/// `None` if the repo lookup / creation failed.
async fn find_or_create_skills_parent(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> Option<Uuid> {
    let all = note_repo.list_for_project(project_id).ok()?;
    if let Some(existing) = all.iter().find(|n| {
        n.parent_id.is_none()
            && n.title == SKILLS_PARENT_TITLE
            && matches!(n.kind, NoteKind::Markdown)
    }) {
        return Some(existing.id);
    }
    let row = note_repo
        .create_with_kind(project_id, None, SKILLS_PARENT_TITLE, NoteKind::Markdown)
        .ok()?;
    // Seed an empty body — `import_skills_from_folder` rewrites the
    // body at the end of the import with the README + auto-list. The
    // seed exists only so opening the note before the first import
    // shows something rather than an empty file.
    let _ = persistence
        .save(&row.id.to_string(), b"# SKILLS\n")
        .await;
    Some(row.id)
}

/// Read the import folder's `README.md` (case-insensitive on the
/// stem and extension) into a string. Returns `None` if no README is
/// present or it can't be read — callers fall back to a minimal
/// auto-generated header.
fn read_folder_readme(folder: &Path) -> Option<String> {
    let entries = std::fs::read_dir(folder).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if stem.eq_ignore_ascii_case("readme") && ext.eq_ignore_ascii_case("md") {
            return std::fs::read_to_string(&path).ok();
        }
    }
    None
}

/// Build the SKILLS index body. Composition: README prose (when the
/// folder shipped one) + an auto-generated `## Imported skills`
/// section listing every skill child as an `operon://note/<uuid>`
/// markdown link the renderer wires up to navigation.
fn build_skills_index_body(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_id: Uuid,
    skills_parent_id: Uuid,
    readme: Option<&str>,
) -> String {
    let all = note_repo.list_for_project(project_id).unwrap_or_default();
    let mut children: Vec<(String, Uuid)> = all
        .into_iter()
        .filter(|n| {
            n.parent_id == Some(skills_parent_id) && matches!(n.kind, NoteKind::Skill)
        })
        .map(|n| (n.title, n.id))
        .collect();
    children.sort_by(|a, b| a.0.cmp(&b.0));
    render_skills_index_body(readme, &children)
}

/// Pure renderer for the SKILLS index body. Split from
/// `build_skills_index_body` so the formatting (preamble, marker
/// lines, fallback when no children exist) is unit-testable without
/// a repo.
fn render_skills_index_body(readme: Option<&str>, children: &[(String, Uuid)]) -> String {
    let mut out = match readme {
        Some(r) => {
            let trimmed = r.trim_end();
            if trimmed.is_empty() {
                String::from("# SKILLS\n")
            } else {
                let mut s = String::with_capacity(trimmed.len() + 64);
                s.push_str(trimmed);
                s.push('\n');
                s
            }
        }
        None => String::from("# SKILLS\n"),
    };
    out.push_str("\n## Imported skills\n\n");
    if children.is_empty() {
        out.push_str("_(no skills imported yet — re-run \"Import skills…\" to add some)_\n");
    } else {
        for (title, id) in children {
            out.push_str(&format!("- [{title}](operon://note/{id})\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_index_lists_children_as_operon_links() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let body = render_skills_index_body(
            None,
            &[("02-discover-epics".into(), id_a), ("03-decompose-features".into(), id_b)],
        );
        assert!(body.contains("# SKILLS"));
        assert!(body.contains("## Imported skills"));
        assert!(body.contains(&format!("- [02-discover-epics](operon://note/{id_a})")));
        assert!(body.contains(&format!("- [03-decompose-features](operon://note/{id_b})")));
    }

    #[test]
    fn skills_index_uses_readme_as_preamble_when_present() {
        let readme = "# Seed skills\n\nThis chain decomposes requirements into tasks.\n";
        let body = render_skills_index_body(Some(readme), &[]);
        assert!(body.starts_with("# Seed skills\n"));
        assert!(body.contains("This chain decomposes requirements"));
        assert!(body.contains("## Imported skills"));
        assert!(body.contains("_(no skills imported yet"));
    }

    #[test]
    fn skills_index_falls_back_to_default_header_when_readme_blank() {
        let body = render_skills_index_body(Some("   \n\n  "), &[]);
        assert!(body.starts_with("# SKILLS\n"));
        assert!(body.contains("## Imported skills"));
    }

    #[test]
    fn skills_index_separates_readme_from_auto_section() {
        // The README's last line should not bleed into the
        // `## Imported skills` heading. A blank line must sit
        // between them.
        let id = Uuid::new_v4();
        let body = render_skills_index_body(
            Some("# Pipeline\n\nNarrative ends here."),
            &[("02-discover-epics".into(), id)],
        );
        assert!(body.contains("Narrative ends here.\n\n## Imported skills"));
    }
}
