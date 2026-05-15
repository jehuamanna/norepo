//! A single note row inside the explorer panel. Indented by `depth * 16px`,
//! shows a disclosure caret only when it has children, and supports inline
//! rename / context menu (Phase-4: cut/copy/paste, indent/outdent, move-up/down).

use dioxus::prelude::*;
use dioxus::html::HasFileData;
use keyboard_types::Modifiers;
use operon_store::repos::LocalNote;
use uuid::Uuid;

use crate::editor::EditorMode;
use crate::local_mode::explorer::creatable_kind::{build_creatable_menu, CreatableKind};
use crate::local_mode::explorer::{
    extend_keyboard_selection, ExplorerUndoCtx, FocusedNode, LastClicked, MultiSelected, NodeKey,
    NotesByProjectCtx, VisibleFlat,
};
use crate::local_mode::ui::{
    classify_drop_position, ContextMenu, ContextMenuItem, DragDescendants, DragKind, DragSession,
    DropPosition, InlineRename,
};

/// Per-level indent in pixels — matches the `--depth * 12px` formula in
/// `assets/shell.css`'s `.notes-explorer-row` rule. Drop indicators use this
/// to align the horizontal line with the chosen target depth.
const INDENT_PX: f64 = 12.0;

#[derive(Props, Clone, PartialEq)]
pub struct NoteRowProps {
    pub note: LocalNote,
    pub depth: i64,
    pub has_children: bool,
    pub is_open: bool,
    pub selected: bool,
    /// True when this note is open in the currently active tab. Renders as
    /// a left accent bar + bold title to distinguish "open in editor" from
    /// the lighter "explorer click-selection" state.
    pub tab_active: bool,
    /// True when this note has an open tab with unsaved edits. Renders a
    /// leading dot ahead of the title, mirroring the dirty-marker dot on
    /// the tab strip itself.
    pub dirty: bool,
    /// SDLC role bucket inferred from the note's artifact_kind (for
    /// Artifact notes) or numeric title prefix (for Skill notes). Drives
    /// a 3px left accent bar on the row. `None` skips the accent.
    pub role: Option<super::role::Role>,
    /// Frontmatter status for Artifact notes (Pending / Approved /
    /// Dirty / Running / Error / Rejected). Drives a small colored
    /// dot at the row's right edge so users can see workflow progress
    /// at a glance. `None` for non-Artifact notes — no dot.
    pub artifact_status: Option<crate::plugins::artifact::frontmatter::ArtifactStatus>,
    /// Frontmatter `artifact_kind` value for Artifact notes. Used to
    /// (a) gate the inline Play icon to the four eligible kinds —
    /// MasterRequirement / Task / ImplementationPlan / Implementation
    /// — that the cascade runner can dispatch on, and (b) light up
    /// the MasterRequirement visual marker so users can spot the
    /// project's cascade root at a glance. `None` for non-artifact
    /// rows or artifacts whose frontmatter doesn't carry a kind.
    pub artifact_kind: Option<crate::plugins::artifact::frontmatter::ArtifactKind>,
    pub in_rename: bool,
    pub is_first_sibling: bool,
    pub is_last_sibling: bool,
    pub cut: bool,
    pub has_clip_note: bool,
    pub drag_active: bool,
    pub on_select: Callback<Uuid>,
    pub on_toggle_open: Callback<Uuid>,
    pub on_rename: Callback<(Uuid, String)>,
    pub on_request_rename: Callback<Uuid>,
    pub on_request_delete: Callback<Uuid>,
    /// Kind-aware add-child. Submenu leaves dispatch with a
    /// [`CreatableKind`] — `Plain(NoteKind)` for the file-level kinds
    /// (Markdown / Image / etc.) and `Artifact(ArtifactKind)` for the
    /// typed-pipeline submenu under "Artifact ▶". The handler in
    /// `explorer/mod.rs` branches: Plain takes the existing fast path
    /// (Image opens a file picker), Artifact creates an Artifact note
    /// + writes the scaffold body via `Persistence::save`.
    pub on_add_child: Callback<(Uuid, CreatableKind)>,
    /// Kind-aware add-sibling. Same dispatch contract as `on_add_child`.
    /// The Plain branch creates then `move_to(.., target.sibling_index + 1)`;
    /// Image goes via the project-scoped picker plumbing with the target's
    /// `parent_id`; Artifact creates + writes scaffold + repositions.
    pub on_add_sibling: Callback<(Uuid, CreatableKind)>,
    pub on_indent: Callback<Uuid>,
    pub on_outdent: Callback<Uuid>,
    pub on_move_up: Callback<Uuid>,
    pub on_move_down: Callback<Uuid>,
    pub on_cut: Callback<Uuid>,
    pub on_copy: Callback<Uuid>,
    pub on_paste: Callback<Uuid>,
    /// Plans-Phase-3-explorer-drag-drop-feedback: tuple is
    /// (src, target, position, chosen_depth). `chosen_depth` is the depth
    /// the user picked with the cursor's X position relative to the row's
    /// indent baseline — only meaningful for `After` (Notion-style outdent
    /// or indent-into-target). `Into` and `Before` ignore it. The handler
    /// in `explorer/mod.rs` runs `resolve_drop_parent` to translate the
    /// triple into a concrete `(parent_id, sibling_index)` for `move_to`.
    pub on_drop_note_on_note: Callback<(Uuid, Uuid, DropPosition, i64)>,
    /// Plans-Phase-6-image-notes: external image-file drops onto this row
    /// land as child image-notes. Tuple is (parent_note_id, bytes,
    /// suggested filename including extension).
    pub on_drop_image_file: Callback<(Uuid, Vec<u8>, String)>,
    /// Current editor mode for this note when it's open in a tab. None means
    /// the note isn't open yet — picking any mode opens it.
    pub current_mode: Option<EditorMode>,
    /// Switch the note's editor mode (opens the tab if needed).
    pub on_set_mode: Callback<(Uuid, EditorMode)>,
    /// Plans-Phase-4-multiselect-aria: bulk variants invoked from the
    /// context menu when the right-clicked row is itself in
    /// `MultiSelected` (size >= 2). Cut/Copy populate the `BulkClipboard`;
    /// Delete raises the existing `pending_bulk_delete` confirmation flag.
    pub on_bulk_cut: Callback<()>,
    pub on_bulk_copy: Callback<()>,
    pub on_bulk_request_delete: Callback<()>,
    /// Opt this note into the auto-managed Contents section. Loads
    /// the body, appends the toc sentinel + empty Contents block if
    /// absent, and persists. Idempotent — re-invoking is a no-op once
    /// the sentinel is present. Available on any note kind; CE /
    /// Phase / Artifact already carry it from creation.
    pub on_insert_toc: Callback<Uuid>,
}

#[component]
pub fn NoteRow(props: NoteRowProps) -> Element {
    let menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut menu_pos_setter = menu_pos;
    // Operon-Phase-3-note-kind-dropdown: + button on a note row opens a
    // dropdown of every NoteKind so the user can create a child of any
    // kind in one click. Tracked separately from `menu_pos` so the
    // right-click menu and the + dropdown can't collide.
    let add_menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut add_menu_pos_setter = add_menu_pos;
    // Plans-Phase-3-explorer-drag-drop-feedback: tri-state indicator —
    // None = no drag over this row; Some(Ok(pos)) = a valid drop position;
    // Some(Err(())) = a forbidden drop (would create a cycle, or the row
    // is mid-rename). Forbidden renders a "no-drop" border.
    let drop_indicator: Signal<Option<Result<DropPosition, ()>>> = use_signal(|| None);
    let mut drop_indicator_setter = drop_indicator;
    // Plans-Phase-7-snap: when the (target, position) pair is stable for
    // 80 ms the indicator emphasises (thicker line / saturated ring). The
    // generation counter guards against races where the cursor moved away
    // before the timer fired.
    let mut snapped: Signal<bool> = use_signal(|| false);
    let mut hover_generation: Signal<u64> = use_signal(|| 0);
    // Notion-style depth-aware drop: the cursor's X position relative to
    // the row's indent baseline picks a target depth, which (for `After`)
    // selects which ancestor of `target` becomes the new parent. `Before`
    // and `Into` ignore this; the state still tracks it so the visual
    // indent line follows the cursor smoothly during a drag.
    let mut chosen_depth: Signal<i64> = use_signal(|| 0);
    let DragSession(mut drag_session) = use_context();
    // Plans-Phase-3-explorer-drag-drop-feedback: descendant set of the
    // dragged note (populated below by ondragstart) and the panel-scope
    // notes snapshot used to compute it.
    let DragDescendants(mut drag_descendants) = use_context();
    let NotesByProjectCtx(notes_by_project_ctx) = use_context();
    // Plans-Phase-8-explorer-undo: handle to the panel's undo stack +
    // dispatch callback so we can render an "Undo last action" item.
    let ExplorerUndoCtx { history, on_undo, on_redo } = use_context::<ExplorerUndoCtx>();
    // Plans-Phase-4-multiselect-aria
    let MultiSelected(mut multi_selected) = use_context();
    let LastClicked(mut last_clicked) = use_context();
    let VisibleFlat(visible_flat) = use_context();
    let FocusedNode(mut focused_node) = use_context();
    // Space = open without focus shift. We need a handle to the focus
    // request signal so the Space keydown branch can clear it after
    // on_select sets it (see the Space handler below).
    let crate::editor::RequestEditorFocus(focus_request_for_space) =
        use_context();

    let note = props.note.clone();
    let id = note.id;
    let id_str = id.to_string();
    let title = note.title.clone();
    let dirty = props.dirty;
    let role = props.role;
    // The chip is informative only for Artifact notes — Skill notes
    // already carry the role signal in their numeric prefix
    // (01-/07-/…), so a chip there is redundant. The row's role color
    // (title tint, caret tint) still applies to skills.
    let show_role_chip = matches!(note.kind, operon_store::repos::NoteKind::Artifact);
    // Captured by Copy into the keydown closure so ArrowLeft (parent
    // navigation) can write the parent's NodeKey into `focused_node`
    // without re-borrowing `note`.
    let parent_id = note.parent_id;
    let project_id = note.project_id;
    let depth = props.depth.max(0);
    let has_children = props.has_children;
    let is_open = props.is_open;
    // Plans-Phase-4: a row is "selected" for visual + ARIA purposes if
    // either the single-select prop is set OR the multi-selection set
    // contains it.
    let in_multi = multi_selected.read().contains(&NodeKey::Note(id));
    let selected = props.selected || in_multi;
    let in_rename = props.in_rename;
    let is_first_sibling = props.is_first_sibling;
    let is_last_sibling = props.is_last_sibling;
    let cut = props.cut;
    let has_clip_note = props.has_clip_note;
    let drag_active = props.drag_active;

    let on_select = props.on_select;
    let on_toggle_open = props.on_toggle_open;
    let on_rename = props.on_rename;
    let on_request_rename = props.on_request_rename;
    let on_request_delete = props.on_request_delete;
    let on_add_child = props.on_add_child;
    let on_add_sibling = props.on_add_sibling;
    let on_indent = props.on_indent;
    let on_outdent = props.on_outdent;
    let on_move_up = props.on_move_up;
    let on_move_down = props.on_move_down;
    let on_cut = props.on_cut;
    let on_copy = props.on_copy;
    let on_paste = props.on_paste;
    let on_drop_note_on_note = props.on_drop_note_on_note;
    let on_drop_image_file = props.on_drop_image_file;
    let current_mode = props.current_mode;
    let on_set_mode = props.on_set_mode;
    let on_bulk_cut = props.on_bulk_cut;
    let on_bulk_copy = props.on_bulk_copy;
    let on_bulk_request_delete = props.on_bulk_request_delete;
    let on_insert_toc = props.on_insert_toc;
    // Plans-Phase-4-multiselect-aria: when this row is part of the
    // multi-selection AND the set has 2+ items, surface bulk-aware
    // menu labels ("Cut N items" etc.) and route Cut/Copy/Delete
    // through the panel-scope bulk callbacks.
    let bulk_count = multi_selected.read().len();
    let is_bulk = in_multi && bulk_count >= 2;

    let mut row_class = if selected {
        String::from("notes-explorer-row notes-explorer-row-note notes-explorer-row-active group")
    } else {
        String::from("notes-explorer-row notes-explorer-row-note group")
    };
    if props.tab_active {
        row_class.push_str(" notes-explorer-row-tab-active notes-explorer-row-tab-active-note");
    }
    if cut {
        row_class.push_str(" notes-explorer-row-cut");
    }
    if let Some(r) = props.role {
        row_class.push(' ');
        row_class.push_str(r.css_class());
    }
    let style = format!("--depth: {depth};");

    // Freshly-created Markdown notes are persisted with an empty title. Render
    // the inline rename input with "Untitled" so the user has visible text to
    // overwrite (paired with the on-mount select-all in InlineRename).
    let initial_title = if title.is_empty() {
        String::from("Untitled")
    } else {
        title.clone()
    };
    let dismiss_menu = use_callback(move |_: ()| menu_pos_setter.set(None));
    let dismiss_add_menu = use_callback(move |_: ()| add_menu_pos_setter.set(None));

    // The "+" dropdown creates a CHILD note. Built from the shared
    // layout in `creatable_kind.rs` so the same shape (plain kinds +
    // typed-Artifact submenu) appears in every creation surface
    // (right-click "Add child"/"Add sibling", this dropdown, and the
    // project-row "+" dropdown).
    let on_pick_for_add_dropdown: Callback<CreatableKind> = Callback::new(move |kind| {
        on_add_child.call((id, kind));
    });
    let add_menu_items: Vec<ContextMenuItem> = build_creatable_menu(on_pick_for_add_dropdown);

    let mut paste_item = ContextMenuItem::new(
        "Paste",
        Callback::new(move |_| {
            on_paste.call(id);
        }),
    );
    paste_item.enabled = has_clip_note;
    let mut indent_item = ContextMenuItem::new(
        "Indent",
        Callback::new(move |_| {
            on_indent.call(id);
        }),
    );
    indent_item.enabled = !is_first_sibling;
    let mut outdent_item = ContextMenuItem::new(
        "Outdent",
        Callback::new(move |_| {
            on_outdent.call(id);
        }),
    );
    outdent_item.enabled = depth > 0;
    let mut move_up_item = ContextMenuItem::new(
        "Move up",
        Callback::new(move |_| {
            on_move_up.call(id);
        }),
    );
    move_up_item.enabled = !is_first_sibling;
    let mut move_down_item = ContextMenuItem::new(
        "Move down",
        Callback::new(move |_| {
            on_move_down.call(id);
        }),
    );
    move_down_item.enabled = !is_last_sibling;

    // Mode-switch items: render the modes the note is NOT currently in. If
    // the note isn't open yet, all three are offered — picking one opens the
    // note in that mode. Items live first in the menu so they're easy to hit.
    let mut mode_items: Vec<ContextMenuItem> = Vec::new();
    if current_mode != Some(EditorMode::Edit) {
        mode_items.push(ContextMenuItem::new(
            "Revise",
            Callback::new(move |_| {
                on_set_mode.call((id, EditorMode::Edit));
            }),
        ));
    }
    if current_mode != Some(EditorMode::View) {
        mode_items.push(ContextMenuItem::new(
            "View",
            Callback::new(move |_| {
                on_set_mode.call((id, EditorMode::View));
            }),
        ));
    }
    if current_mode != Some(EditorMode::Split) {
        mode_items.push(ContextMenuItem::new(
            "Split view",
            Callback::new(move |_| {
                on_set_mode.call((id, EditorMode::Split));
            }),
        ));
    }

    let id_for_copy = id_str.clone();
    let mut menu_items: Vec<ContextMenuItem> = mode_items;
    // "Send to chat" appends a `@[<title>](note:<uuid>)` mention to
    // the companion chat composer. Pulled from the optional
    // `CompanionComposerAppend` context — skipped silently when the
    // companion isn't mounted (e.g. tests, vault-less standalone).
    let composer_append_handle =
        try_consume_context::<crate::shell::companion_state::CompanionComposerAppend>()
            .map(|c| c.0);
    if let Some(mut append_sig) = composer_append_handle {
        let title_for_chat = title.clone();
        menu_items.push(ContextMenuItem::new(
            "Send to chat",
            Callback::new(move |_| {
                let token = format!("@[{}](note:{})", title_for_chat, id);
                append_sig.set(Some(token));
            }),
        ));
    }
    menu_items.extend([
        ContextMenuItem::new(
            "Rename",
            Callback::new(move |_| {
                on_request_rename.call(id);
            }),
        ),
        ContextMenuItem::new(
            "Copy ID",
            Callback::new(move |_| {
                crate::util::clipboard::copy_text(&id_for_copy);
            }),
        ),
        ContextMenuItem::submenu(
            "Add child note",
            build_creatable_menu(Callback::new(move |kind| {
                on_add_child.call((id, kind));
            })),
        ),
        ContextMenuItem::submenu(
            "Add sibling note",
            build_creatable_menu(Callback::new(move |kind| {
                on_add_sibling.call((id, kind));
            })),
        ),
        ContextMenuItem::new(
            "Insert Contents",
            Callback::new(move |_| {
                on_insert_toc.call(id);
            }),
        ),
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
        indent_item,
        outdent_item,
        move_up_item,
        move_down_item,
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
            // when the redo deque is empty (e.g. fresh session, or after a
            // user gesture invalidated the redo path).
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
    ]);

    let caret_glyph = if has_children {
        if is_open {
            "\u{25BE}"
        } else {
            "\u{25B8}"
        }
    } else {
        ""
    };

    let drop_pos_now = *drop_indicator.read();

    let aria_level = (depth + 2).max(2); // projects are level 1; root notes level 2
    rsx! {
        div {
            class: "{row_class}",
            style: "{style}",
            "data-testid": "note-row",
            "data-explorer": "true",
            "data-note-id": "{id_str}",
            "data-note-depth": "{depth}",
            "data-selected": if selected { "true" } else { "false" },
            "data-open": if is_open { "true" } else { "false" },
            "data-cut": if cut { "true" } else { "false" },
            // Plans-Phase-4-multiselect-aria: WAI-ARIA tree pattern.
            role: "treeitem",
            "aria-level": "{aria_level}",
            "aria-selected": if selected { "true" } else { "false" },
            "aria-expanded": if has_children { if is_open { "true" } else { "false" } } else { "" },
            tabindex: "0",
            draggable: "true",
            onclick: move |evt| {
                evt.stop_propagation();
                let mods = evt.modifiers();
                let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                let key = NodeKey::Note(id);
                if with_meta && !mods.contains(Modifiers::SHIFT) {
                    // Plans-Phase-4: Ctrl/Cmd+click toggles in the multi-set
                    // without disturbing the single-select signal.
                    multi_selected.with_mut(|set| {
                        if !set.remove(&key) {
                            set.insert(key);
                        }
                    });
                    last_clicked.set(Some(key));
                    return;
                }
                if mods.contains(Modifiers::SHIFT) {
                    // Plans-Phase-4: full range over the visible flat tree.
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
                            // Either endpoint isn't in the visible flat
                            // (e.g. it just collapsed); fall back to the
                            // endpoint union.
                            set.insert(prev);
                            set.insert(key);
                        }
                    } else {
                        set.insert(key);
                    }
                    multi_selected.set(set);
                    return;
                }
                // Plain click: clear multi-set, fall through to single select.
                if !multi_selected.read().is_empty() {
                    multi_selected.set(std::collections::BTreeSet::new());
                }
                last_clicked.set(Some(key));
                let mut focus_req = focus_request_for_space;
                on_select.call(id);
                focus_req.set(None);
                focused_node.set(Some(NodeKey::Note(id)));
                focus_explorer_node_deferred(NodeKey::Note(id));
            },
            ondoubleclick: move |evt| {
                evt.stop_propagation();
                if has_children {
                    on_toggle_open.call(id);
                }
            },
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                let coords = evt.client_coordinates();
                menu_pos_setter.set(Some((coords.x as i32, coords.y as i32)));
            },
            onkeydown: {
                let id_for_keys = id_str.clone();
                move |evt| {
                    let key = evt.key().to_string();
                    let mods = evt.modifiers();
                    let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                    if with_meta && mods.contains(Modifiers::SHIFT) && !mods.contains(Modifiers::ALT)
                        && key.eq_ignore_ascii_case("c")
                    {
                        // Plans-Phase-3-note-id-create: Cmd/Ctrl+Shift+C copies the
                        // focused row's note id to the clipboard.
                        evt.prevent_default();
                        evt.stop_propagation();
                        crate::util::clipboard::copy_text(&id_for_keys);
                        return;
                    }
                    // Plans-Phase-11: row-level Cmd/Ctrl+Z + Shift variant
                    // for undo / redo. The panel root carries the same
                    // listener via event bubbling, but in some WebView
                    // builds keydown does not bubble out of a focused
                    // tabindex=0 child reliably — handling it here as
                    // well is cheap and makes the shortcut work whenever
                    // a row has focus. We skip while in inline-rename so
                    // the input keeps native text-undo.
                    if with_meta && !mods.contains(Modifiers::ALT)
                        && key.eq_ignore_ascii_case("z")
                        && !in_rename
                    {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if mods.contains(Modifiers::SHIFT) {
                            on_redo.call(());
                        } else {
                            on_undo.call(());
                        }
                        return;
                    }
                    if key == "ArrowUp" && mods.contains(Modifiers::ALT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_move_up.call(id);
                        // Same NodeKey, but the data mutation reorders the
                        // DOM; the deferred JS focus re-asserts DOM focus
                        // after the diff applies (double-RAF), keeping
                        // held-down Alt+↑ continuous.
                        focused_node.set(Some(NodeKey::Note(id)));
                        focus_explorer_node_deferred(NodeKey::Note(id));
                    } else if key == "ArrowDown" && mods.contains(Modifiers::ALT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_move_down.call(id);
                        focused_node.set(Some(NodeKey::Note(id)));
                        focus_explorer_node_deferred(NodeKey::Note(id));
                    } else if key == "ArrowLeft" && mods.contains(Modifiers::ALT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_outdent.call(id);
                        focused_node.set(Some(NodeKey::Note(id)));
                        focus_explorer_node_deferred(NodeKey::Note(id));
                    } else if key == "ArrowRight" && mods.contains(Modifiers::ALT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_indent.call(id);
                        focused_node.set(Some(NodeKey::Note(id)));
                        focus_explorer_node_deferred(NodeKey::Note(id));
                    } else if key == "ArrowDown" && mods.contains(Modifiers::SHIFT) {
                        // Shift+ArrowDown: extend the multi-selection one row
                        // downward from the current anchor (last_clicked).
                        evt.prevent_default();
                        evt.stop_propagation();
                        extend_keyboard_selection(
                            NodeKey::Note(id),
                            1,
                            &mut multi_selected,
                            &last_clicked,
                            &visible_flat,
                        );
                        if let Some(next) = next_visible(NodeKey::Note(id), 1, &visible_flat) {
                            focused_node.set(Some(next));
                            focus_explorer_node_deferred(next);
                        }
                    } else if key == "ArrowUp" && mods.contains(Modifiers::SHIFT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        extend_keyboard_selection(
                            NodeKey::Note(id),
                            -1,
                            &mut multi_selected,
                            &last_clicked,
                            &visible_flat,
                        );
                        if let Some(next) = next_visible(NodeKey::Note(id), -1, &visible_flat) {
                            focused_node.set(Some(next));
                            focus_explorer_node_deferred(next);
                        }
                    } else if key == "ArrowDown" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if let Some(next) = next_visible(NodeKey::Note(id), 1, &visible_flat) {
                            focused_node.set(Some(next));
                            focus_explorer_node_deferred(next);
                        }
                    } else if key == "ArrowUp" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if let Some(next) = next_visible(NodeKey::Note(id), -1, &visible_flat) {
                            focused_node.set(Some(next));
                            focus_explorer_node_deferred(next);
                        }
                    } else if key == "ArrowRight" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if has_children {
                            if !is_open {
                                on_toggle_open.call(id);
                            } else if let Some(next) =
                                next_visible(NodeKey::Note(id), 1, &visible_flat)
                            {
                                focused_node.set(Some(next));
                                focus_explorer_node_deferred(next);
                            }
                        }
                    } else if key == "ArrowLeft" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if has_children && is_open {
                            on_toggle_open.call(id);
                        } else {
                            // Parent: NodeKey::Note(parent_id) if nested,
                            // otherwise the project row (NodeKey::Project).
                            let parent_key = parent_id
                                .map(NodeKey::Note)
                                .unwrap_or(NodeKey::Project(project_id));
                            focused_node.set(Some(parent_key));
                            focus_explorer_node_deferred(parent_key);
                        }
                    } else if key == "Home" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        let flat = visible_flat.peek().clone();
                        if let Some(first) = flat.first().copied() {
                            focused_node.set(Some(first));
                            focus_explorer_node_deferred(first);
                        }
                    } else if key == "End" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        let flat = visible_flat.peek().clone();
                        if let Some(last) = flat.last().copied() {
                            focused_node.set(Some(last));
                            focus_explorer_node_deferred(last);
                        }
                    } else if key == "Enter" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        // Enter = open + focus. `on_select` already writes
                        // `RequestEditorFocus`; the desktop MonacoEditorHost
                        // (editor_host.rs) and the wasm path both consume
                        // it after mount.
                        on_select.call(id);
                    } else if key == " " {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if has_children {
                            on_toggle_open.call(id);
                        } else {
                            // Space = open WITHOUT shifting focus. We let
                            // on_select run (so the tab opens / activates),
                            // then immediately clear the focus request so
                            // the editor host's use_effect sees `None` and
                            // skips its focus dispatch. Both writes happen
                            // in the same render tick — Dioxus collapses
                            // them and the effect reads only the final
                            // value.
                            let mut focus_req = focus_request_for_space;
                            on_select.call(id);
                            focus_req.set(None);
                        }
                    } else if key == "F2" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_request_rename.call(id);
                    } else if (key == "Delete" || key == "Backspace")
                        && multi_selected.read().len() < 2
                    {
                        // Single-row delete via keyboard. The bulk path is
                        // handled at the panel root for selection size >= 2.
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_request_delete.call(id);
                    }
                }
            },
            ondragstart: move |_| {
                drag_session.set(Some(DragKind::Note(id)));
                // Plans-Phase-3-explorer-drag-drop-feedback: precompute the
                // descendant set so dragover can reject cycle-creating drops.
                let snap = notes_by_project_ctx.read();
                let mut descendants: std::collections::BTreeSet<Uuid> =
                    std::collections::BTreeSet::new();
                if let Some((_, list)) = snap
                    .iter()
                    .find(|(_, list)| list.iter().any(|n| n.id == id))
                {
                    let mut frontier: Vec<Uuid> = vec![id];
                    while let Some(parent) = frontier.pop() {
                        for n in list.iter().filter(|n| n.parent_id == Some(parent)) {
                            if descendants.insert(n.id) {
                                frontier.push(n.id);
                            }
                        }
                    }
                }
                drag_descendants.set(descendants);
            },
            ondragend: move |_| {
                drag_session.set(None);
                drop_indicator_setter.set(None);
                drag_descendants.set(std::collections::BTreeSet::new());
                snapped.set(false);
                hover_generation.with_mut(|g| *g = g.wrapping_add(1));
                chosen_depth.set(0);
            },
            ondragover: move |evt| {
                evt.prevent_default();
                let kind = *drag_session.read();
                let coords = evt.element_coordinates();
                let pos = classify_drop_position(coords.y, 28.0);
                // Notion-style depth chooser, anchored at target's own indent.
                // The row's content sits at `depth * 12px + 6px` of left
                // padding, so the *natural* cursor position (over the row's
                // text or anywhere right of the indent baseline) defaults to
                // sibling-at-target-depth — what users expect for a simple
                // reorder. Only when the user deliberately pulls the cursor
                // *left* of that anchor does the chosen depth step down by
                // one per `INDENT_PX` of leftward travel. We never auto-
                // indent past the target's depth; users who want "first
                // child" can drop Into the target instead.
                let target_anchor_x = depth as f64 * INDENT_PX;
                let dx = coords.x - target_anchor_x;
                let new_chosen_depth = if dx < 0.0 {
                    let outdent_steps = ((-dx) / INDENT_PX).ceil() as i64;
                    (depth - outdent_steps).max(0)
                } else {
                    depth
                };
                let prev_chosen_depth = *chosen_depth.read();
                if prev_chosen_depth != new_chosen_depth {
                    chosen_depth.set(new_chosen_depth);
                }
                // Plans-Phase-3: classify the drop as
                //   - Some(Ok(pos))  → allowed; show coloured indicator
                //   - Some(Err(()))  → forbidden; show no-drop indicator
                //   - None           → not a note drag we care about
                let next = match kind {
                    Some(DragKind::Note(src)) => {
                        if in_rename {
                            // Don't disrupt a row mid-rename.
                            None
                        } else if src == id || drag_descendants.read().contains(&id) {
                            Some(Err(()))
                        } else {
                            // Plans-Phase-4-multiselect-aria: if the drag
                            // source is part of a 2+ multi-set, every member
                            // must share the same `SiblingGroup` (same
                            // parent_id, or all WorkspaceRoot for projects).
                            // A non-sibling set surfaces a forbidden
                            // indicator so users see the rejection during
                            // drag.
                            let multi_snap = multi_selected.read().clone();
                            if multi_snap.contains(&NodeKey::Note(src)) && multi_snap.len() >= 2 {
                                let notes_snap = notes_by_project_ctx.read();
                                if !crate::local_mode::explorer::all_siblings(&multi_snap, &notes_snap) {
                                    Some(Err(()))
                                } else {
                                    Some(Ok(pos))
                                }
                            } else {
                                Some(Ok(pos))
                            }
                        }
                    }
                    _ => None,
                };
                let prev = *drop_indicator.read();
                // Reset snap when the indent line shifts horizontally too —
                // a cursor that's still moving horizontally hasn't "settled".
                let depth_changed = prev_chosen_depth != new_chosen_depth;
                if prev != next || depth_changed {
                    if prev != next {
                        drop_indicator_setter.set(next);
                    }
                    // Plans-Phase-7-snap: bumping the generation invalidates
                    // any in-flight snap or hover-expand timer for the prior
                    // (target, pos).
                    snapped.set(false);
                    let gen = hover_generation.with_mut(|g| {
                        *g = g.wrapping_add(1);
                        *g
                    });
                    // Only arm timers for *valid* drop positions.
                    if matches!(next, Some(Ok(_))) {
                        let captured_gen = gen;
                        let captured_next = next;
                        spawn(async move {
                            futures_timer::Delay::new(
                                std::time::Duration::from_millis(80),
                            )
                            .await;
                            let still_current = *hover_generation.read() == captured_gen
                                && *drop_indicator.read() == captured_next;
                            if still_current {
                                snapped.set(true);
                            }
                        });
                        // Plans-Phase-7-hover-expand: if the user is hovering
                        // the Into zone of a collapsed parent for ≥600 ms,
                        // auto-expand so they can drop into a descendant.
                        if matches!(next, Some(Ok(DropPosition::Into))) && has_children && !is_open {
                            let captured_gen_expand = gen;
                            let captured_next_expand = next;
                            spawn(async move {
                                futures_timer::Delay::new(
                                    std::time::Duration::from_millis(600),
                                )
                                .await;
                                let still_current = *hover_generation.read() == captured_gen_expand
                                    && *drop_indicator.read() == captured_next_expand;
                                if still_current {
                                    on_toggle_open.call(id);
                                }
                            });
                        }
                    }
                }
            },
            ondragleave: move |_| {
                drop_indicator_setter.set(None);
                snapped.set(false);
                hover_generation.with_mut(|g| *g = g.wrapping_add(1));
            },
            ondrop: move |evt| {
                evt.prevent_default();
                // An in-app drag (note row in this explorer) wins over any
                // payload the browser may have stuffed into
                // `evt.data().files()`. Some image-bearing in-app drags
                // surface there too (Dioxus desktop / wry shim), and
                // letting the file-drop branch run for those caused image
                // notes to be re-imported under the file's stem instead
                // of moved. `drag_session` is `Some` iff an in-app drag
                // is in flight, so it is the right discriminator.
                let kind = *drag_session.read();
                if kind.is_none() {
                    // No in-app drag → treat as an OS file drop. Image
                    // files mint new child image-notes under this row;
                    // anything else is ignored.
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
                // Recompute depth from the drop's own coords — the dragover
                // signal can lag by a frame. Same anchor-relative rule as
                // ondragover: default sibling-at-target-depth, outdent only
                // on deliberate leftward drag.
                let target_anchor_x = depth as f64 * INDENT_PX;
                let dx = coords.x - target_anchor_x;
                let drop_depth = if dx < 0.0 {
                    let outdent_steps = ((-dx) / INDENT_PX).ceil() as i64;
                    (depth - outdent_steps).max(0)
                } else {
                    depth
                };
                // Plans-Phase-3-explorer-drag-drop-feedback: refuse drops on
                // a row mid-rename or when the target is a descendant of
                // the source. The visual indicator already reflects this
                // — here we just suppress the repo call.
                let descendants_snap = drag_descendants.read().clone();
                if let Some(DragKind::Note(src)) = kind {
                    if in_rename || src == id || descendants_snap.contains(&id) {
                        drag_session.set(None);
                        drop_indicator_setter.set(None);
                        drag_descendants.set(std::collections::BTreeSet::new());
                        return;
                    }
                    // Plans-Phase-4-multiselect-aria: if the drag source is
                    // a member of the multi-set, drop the whole set at the
                    // target — but only when every member shares the same
                    // `SiblingGroup`. A mixed-parent (or note+project) set
                    // already showed a forbidden indicator during dragover;
                    // reject the drop here too so behavior matches.
                    let set_snap = multi_selected.read().clone();
                    let multi_drop = set_snap.contains(&NodeKey::Note(src)) && set_snap.len() >= 2;
                    if multi_drop {
                        let notes_snap = notes_by_project_ctx.read();
                        if !crate::local_mode::explorer::all_siblings(&set_snap, &notes_snap) {
                            drag_session.set(None);
                            drop_indicator_setter.set(None);
                            drag_descendants.set(std::collections::BTreeSet::new());
                            snapped.set(false);
                            hover_generation.with_mut(|g| *g = g.wrapping_add(1));
                            chosen_depth.set(0);
                            return;
                        }
                        drop(notes_snap);
                        for k in set_snap.iter() {
                            if let NodeKey::Note(n_id) = k {
                                if *n_id != id && !descendants_snap.contains(n_id) {
                                    on_drop_note_on_note.call((*n_id, id, pos, drop_depth));
                                }
                            }
                        }
                    } else if src != id {
                        on_drop_note_on_note.call((src, id, pos, drop_depth));
                    }
                }
                drag_session.set(None);
                drop_indicator_setter.set(None);
                drag_descendants.set(std::collections::BTreeSet::new());
                snapped.set(false);
                hover_generation.with_mut(|g| *g = g.wrapping_add(1));
                chosen_depth.set(0);
            },
            // Plans-Phase-3-note-id-create: leading grip glyph as a visible
            // indicator that the row is draggable. Drag itself is still
            // initiated on the row outer (HTML5 `draggable` attr) so that
            // existing DnD muscle memory keeps working; the glyph is purely a
            // visual affordance.
            span {
                class: "inline-flex w-3 shrink-0 select-none text-xs opacity-0 group-hover:opacity-50",
                "data-testid": "drag-handle",
                "aria-hidden": "true",
                "\u{2807}\u{2807}"
            }
            // Disclosure caret. ≥16x16 hit area (w-4 h-4) so it's reliably
            // hittable; aria-expanded reflects the open/closed state for
            // assistive tech.
            span {
                class: "inline-flex items-center justify-center w-4 h-4 shrink-0 select-none text-xs opacity-70",
                "data-testid": "disclosure-caret",
                "data-has-children": if has_children { "true" } else { "false" },
                role: "button",
                "aria-label": "Toggle children",
                "aria-expanded": if has_children { if is_open { "true" } else { "false" } } else { "false" },
                onclick: move |evt| {
                    evt.stop_propagation();
                    if has_children {
                        on_toggle_open.call(id);
                    }
                },
                "{caret_glyph}"
            }
            // SDLC role chip (BA / SA / SDE). Sits between the disclosure
            // caret and the kind badge so the role color is the first
            // colored token on the row at any depth — the previous left-
            // border accent was getting hidden behind padding on deep
            // rows. The chip's background carries the role color; the
            // text is the two/three-letter role code. Suppressed on
            // Skill notes (their numeric prefix already encodes role).
            if let Some(r) = role.filter(|_| show_role_chip) {
                {
                    let chip_class = r.css_class();
                    let chip_label = match r {
                        super::role::Role::Ba => "BA",
                        super::role::Role::Sa => "SA",
                        super::role::Role::Sde => "SDE",
                    };
                    rsx! {
                        span {
                            class: "operon-role-chip {chip_class}",
                            "data-testid": "role-chip",
                            "data-role": "{chip_label}",
                            title: "{chip_label}",
                            "{chip_label}"
                        }
                    }
                }
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
                    initial: initial_title.clone(),
                    on_commit: Callback::new(move |new_title: String| {
                        on_rename.call((id, new_title));
                    }),
                    on_cancel: Callback::new(move |_| {
                        on_rename.call((id, String::new()));
                    }),
                }
            } else {
                // Plans-Phase-6-image-notes: kind indicator. `[md]` for
                // markdown, `[im]` for image. Drives off `note.kind` so it
                // updates as soon as the row is rendered with a refreshed
                // record.
                if dirty {
                    span {
                        class: "operon-note-dirty-dot",
                        "data-testid": "note-row-dirty-dot",
                        "aria-label": "Unsaved changes",
                        title: "Unsaved changes",
                        "\u{2022}"
                    }
                }
                {
                    // Master requirement is the project's cascade root —
                    // the user's primary entry point. Distinguish it from
                    // every other artifact (which all render `[ar]`) with
                    // an `[mr]` badge and a bolder background, mirroring
                    // the precedent set by Phase notes (`.kind-phase`).
                    let is_master_requirement = matches!(
                        props.artifact_kind,
                        Some(crate::plugins::artifact::frontmatter::ArtifactKind::MasterRequirement)
                    );
                    let (icon, kind_class, kind_attr) = if is_master_requirement {
                        ("mr", "kind-artifact-master", "artifact-master")
                    } else {
                        (note.kind.icon(), "kind-other", note.kind.as_str())
                    };
                    // For non-MR rows we still want the existing per-kind
                    // accent class (`kind-artifact`, `kind-skill`, …) to
                    // apply; for MR we override with the master class only.
                    let kind_class_attr = if is_master_requirement {
                        kind_class.to_string()
                    } else {
                        format!("kind-{}", note.kind.as_str())
                    };
                    rsx! {
                        span {
                            class: "kind-badge {kind_class_attr} text-[0.65rem] mr-1 px-1 rounded select-none opacity-60 shrink-0",
                            "data-testid": "kind-badge",
                            "data-note-kind": "{kind_attr}",
                            "[{icon}]"
                        }
                    }
                }
                span {
                    class: "truncate flex-1",
                    "data-testid": "note-row-name",
                    "{title}"
                }
                // Inline action icons for artifact notes only. Each
                // hits persistence directly without opening the
                // artifact's tab. Position: between the title and the
                // status dot. Mirrors the toolbar's Approve / Reject /
                // Mark dirty buttons + the mode-toggle that the
                // right-click menu also exposes.
                //
                // Icons render only when the row carries an
                // `artifact_status` prop, which the parent panel sets
                // exclusively for `NoteKind::Artifact` rows.
                if let Some(current_status) = props.artifact_status {
                    {
                        let id_for_actions = id;
                        let is_approved = current_status
                            == crate::plugins::artifact::frontmatter::ArtifactStatus::Approved;
                        let dirty_disabled = current_status
                            == crate::plugins::artifact::frontmatter::ArtifactStatus::Dirty;
                        // `current_mode` is `Option<EditorMode>` — None when
                        // the note isn't open. Treat None as not-in-edit so
                        // the icon shows the pencil (entering Revise). When
                        // in Edit, Revise splits into two icons: ✓ Done
                        // (commit) and ✕ Cancel (revert) — mirrors the
                        // in-view artifact toolbar's Done+Cancel pair.
                        let in_edit_mode = matches!(current_mode, Some(EditorMode::Edit));
                        // Combined Approve/Reject: shows the OPPOSITE
                        // of the current state, so one click always
                        // flips the status. Mirrors the artifact view
                        // toolbar's combined button.
                        let approve_reject_label = if is_approved { "\u{2715}" } else { "\u{2713}" };
                        let approve_reject_title = if is_approved { "Reject" } else { "Approve" };
                        let approve_reject_class = if is_approved {
                            "operon-row-action operon-row-action-reject"
                        } else {
                            "operon-row-action operon-row-action-approve"
                        };
                        let approve_reject_target = if is_approved {
                            crate::plugins::artifact::frontmatter::ArtifactStatus::Rejected
                        } else {
                            crate::plugins::artifact::frontmatter::ArtifactStatus::Approved
                        };
                        // Cascade running state for this artifact. The
                        // global CASCADE_STATE map is keyed by uuid;
                        // an entry exists iff a cascade rooted on this
                        // artifact is in flight.
                        let is_cascading = matches!(
                            crate::shell::companion_state::CASCADE_STATE.read().get(&id),
                            Some(crate::shell::companion_state::CascadePhase::Running { .. })
                        );
                        // Play eligibility mirrors the artifact view's
                        // `primary_play_mode` gate at view.rs:238-258:
                        //   - MasterRequirement → always (root seed)
                        //   - Task → Approved or Dirty
                        //   - ImplementationPlan → Approved or Dirty
                        //   - Implementation → Approved or Dirty
                        // Every other kind hides Play. Stop stays
                        // reachable when a cascade is somehow running
                        // for an ineligible kind (defensive — keeps
                        // the user able to cancel).
                        let play_eligible = match (
                            props.artifact_kind.as_ref(),
                            current_status,
                        ) {
                            (
                                Some(crate::plugins::artifact::frontmatter::ArtifactKind::MasterRequirement),
                                _,
                            ) => true,
                            (
                                Some(crate::plugins::artifact::frontmatter::ArtifactKind::Task),
                                crate::plugins::artifact::frontmatter::ArtifactStatus::Approved
                                | crate::plugins::artifact::frontmatter::ArtifactStatus::Dirty,
                            ) => true,
                            (
                                Some(crate::plugins::artifact::frontmatter::ArtifactKind::ImplementationPlan),
                                crate::plugins::artifact::frontmatter::ArtifactStatus::Approved
                                | crate::plugins::artifact::frontmatter::ArtifactStatus::Dirty,
                            ) => true,
                            (
                                Some(crate::plugins::artifact::frontmatter::ArtifactKind::Implementation),
                                crate::plugins::artifact::frontmatter::ArtifactStatus::Approved
                                | crate::plugins::artifact::frontmatter::ArtifactStatus::Dirty,
                            ) => true,
                            _ => false,
                        };
                        let show_play = play_eligible || is_cascading;
                        let play_class = if is_cascading {
                            "operon-row-action operon-row-action-stop"
                        } else {
                            "operon-row-action operon-row-action-play"
                        };
                        let play_title = if is_cascading {
                            "Stop cascade"
                        } else {
                            "Open artifact (Play from the header)"
                        };
                        rsx! {
                            div {
                                class: "operon-row-actions",
                                "data-testid": "row-actions",
                                if show_play {
                                    button {
                                        r#type: "button",
                                        class: "{play_class}",
                                        "data-testid": "row-play",
                                        title: "{play_title}",
                                        onclick: move |evt| {
                                            evt.stop_propagation();
                                            if is_cascading {
                                                if let Some(tok) =
                                                    crate::shell::companion_state::CASCADE_CANCEL
                                                        .read()
                                                        .get(&id_for_actions)
                                                        .cloned()
                                                {
                                                    tok.cancel();
                                                }
                                            } else {
                                                on_select.call(id_for_actions);
                                                on_set_mode.call((id_for_actions, EditorMode::View));
                                            }
                                        },
                                        if is_cascading {
                                            span {
                                                class: "operon-cascade-spinner",
                                                "aria-hidden": "true",
                                            }
                                            "\u{23F9}"
                                        } else {
                                            "\u{25B6}"
                                        }
                                    }
                                }
                                button {
                                    r#type: "button",
                                    class: "{approve_reject_class}",
                                    "data-testid": "row-approve-reject",
                                    title: "{approve_reject_title}",
                                    onclick: move |evt| {
                                        evt.stop_propagation();
                                        apply_row_status(id_for_actions, approve_reject_target);
                                    },
                                    "{approve_reject_label}"
                                }
                                button {
                                    r#type: "button",
                                    class: "operon-row-action operon-row-action-dirty",
                                    "data-testid": "row-mark-dirty",
                                    title: "Mark dirty",
                                    disabled: dirty_disabled,
                                    onclick: move |evt| {
                                        evt.stop_propagation();
                                        apply_row_status(
                                            id_for_actions,
                                            crate::plugins::artifact::frontmatter::ArtifactStatus::Dirty,
                                        );
                                    },
                                    "\u{25D0}"
                                }
                                if in_edit_mode {
                                    button {
                                        r#type: "button",
                                        class: "operon-row-action operon-row-action-done",
                                        "data-testid": "row-revise-done",
                                        title: "Done (commit revision)",
                                        onclick: move |evt| {
                                            evt.stop_propagation();
                                            apply_row_revise_done(id_for_actions);
                                        },
                                        "\u{2713}"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "operon-row-action operon-row-action-cancel",
                                        "data-testid": "row-revise-cancel",
                                        title: "Cancel (revert to pre-Revise state)",
                                        onclick: move |evt| {
                                            evt.stop_propagation();
                                            apply_row_revise_cancel(id_for_actions);
                                        },
                                        "\u{2716}"
                                    }
                                } else {
                                    button {
                                        r#type: "button",
                                        class: "operon-row-action operon-row-action-revise",
                                        "data-testid": "row-revise",
                                        title: "Revise",
                                        onclick: move |evt| {
                                            evt.stop_propagation();
                                            enter_row_revise(id_for_actions, on_set_mode);
                                        },
                                        "\u{270E}"
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(s) = props.artifact_status {
                    {
                        let status_str = s.as_str();
                        rsx! {
                            span {
                                class: "operon-status-dot operon-status-{status_str}",
                                "data-testid": "artifact-status-dot",
                                "data-status": "{status_str}",
                                title: "Status: {status_str}",
                                "aria-label": "Status: {status_str}",
                            }
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "opacity-0 group-hover:opacity-100 inline-flex items-center justify-center w-5 h-5 rounded text-xs hover:bg-[var(--operon-border)]",
                    "data-testid": "add-note-button",
                    "data-note-id": "{id_str}",
                    "aria-label": "Add child note",
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
                {
                    let snap_now = *snapped.read();
                    let chosen_now = *chosen_depth.read();
                    match drop_pos_now {
                        Some(Ok(p)) => {
                            // Resolver ignores chosen_depth for Before/Into,
                            // so pin those indicators to a stable position
                            // (row's own depth) — only After shifts with X.
                            let effective_depth = match p {
                                DropPosition::Before => depth,
                                DropPosition::Into => 0,
                                DropPosition::After => chosen_now,
                            };
                            rsx! { DropIndicator { position: p, snapped: snap_now, depth: effective_depth } }
                        }
                        Some(Err(())) => rsx! { ForbiddenIndicator {} },
                        None => rsx! {},
                    }
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
fn DropIndicator(position: DropPosition, snapped: bool, depth: i64) -> Element {
    // Plans-Phase-7-snap: when `snapped` is true (cursor stable for ≥80 ms),
    // emphasise the indicator — thicker line for Before/After, ring-4 for
    // Into. The data-testid suffixes `-snap` so e2e specs can assert the
    // transition.
    //
    // Notion-style depth offset: for Before/After, the line starts at
    // `depth * 12px` from the row's left edge so it visually aligns with
    // the indent column the user picked with their cursor X. Into still
    // rings the entire row.
    let (testid, class) = match (position, snapped) {
        (DropPosition::Before, false) => (
            "drop-indicator-before",
            "absolute right-0 top-0 h-0.5 bg-[var(--operon-accent)]",
        ),
        (DropPosition::Before, true) => (
            "drop-indicator-before-snap",
            "absolute right-0 top-0 h-1 bg-[var(--operon-accent)]",
        ),
        (DropPosition::Into, false) => (
            "drop-indicator-into",
            "absolute inset-0 ring-2 ring-[var(--operon-accent)] pointer-events-none",
        ),
        (DropPosition::Into, true) => (
            "drop-indicator-into-snap",
            "absolute inset-0 ring-4 ring-[var(--operon-accent)] pointer-events-none",
        ),
        (DropPosition::After, false) => (
            "drop-indicator-after",
            "absolute right-0 bottom-0 h-0.5 bg-[var(--operon-accent)]",
        ),
        (DropPosition::After, true) => (
            "drop-indicator-after-snap",
            "absolute right-0 bottom-0 h-1 bg-[var(--operon-accent)]",
        ),
    };
    let style = match position {
        DropPosition::Into => String::new(),
        _ => format!("left: {}px;", (depth as f64 * INDENT_PX) as i64),
    };
    rsx! {
        span {
            class: "{class}",
            style: "{style}",
            "data-testid": "{testid}",
            "data-drop-depth": "{depth}",
        }
    }
}

/// Defer a DOM focus to *after* Dioxus's diff has applied. Uses a
/// double-`requestAnimationFrame`: the first RAF runs before the next
/// paint (mutations applied), the second guarantees we're past the
/// diff-paint cycle. We don't use `setTimeout(0)` because that fires on
/// the next event-loop tick, which can land *before* Dioxus finishes
/// applying the keyed-list move. We don't use `MountedData::set_focus`
/// because Dioxus 0.7.1's keyed-list reorder is a *move* — `onmounted`
/// doesn't re-fire and the cached handle silently no-ops in Wry's
/// interpreter. We don't use a panel-scope `use_effect` because
/// `use_effect` is documented-broken in `ExplorerPanel`'s render scope
/// (`mod.rs:379-416`). Querying by `data-note-id` / `data-project-id`
/// inside a double-RAF works around all three pitfalls.
pub(super) fn focus_explorer_node_deferred(target: NodeKey) {
    let attr = match target {
        NodeKey::Project(id) => format!(r#"data-project-id="{id}""#),
        NodeKey::Note(id) => format!(r#"data-note-id="{id}""#),
    };
    let script = format!(
        r#"
        requestAnimationFrame(function() {{
            requestAnimationFrame(function() {{
                var el = document.querySelector(
                    '[data-testid="explorer-panel"] [{attr}]'
                );
                if (el && typeof el.focus === 'function') el.focus();
            }});
        }});
        "#
    );
    let _ = document::eval(&script);
}

/// Resolve the next/previous visible explorer row from `visible_flat`,
/// wrapping at the ends. `dir` is `1` for down, `-1` for up. Returns
/// `None` when the list is empty (callers should leave `focused_node`
/// unchanged in that case). Pure Rust — no JS / DOM access.
pub(super) fn next_visible(
    current: NodeKey,
    dir: i32,
    visible_flat: &Signal<Vec<NodeKey>>,
) -> Option<NodeKey> {
    let flat = visible_flat.peek().clone();
    if flat.is_empty() {
        return None;
    }
    let cur_pos = flat
        .iter()
        .position(|k| k == &current)
        .map(|p| p as i32)
        .unwrap_or(0);
    let len = flat.len() as i32;
    let next_pos = ((cur_pos + dir) % len + len) % len;
    Some(flat[next_pos as usize])
}

/// Plans-Phase-3-explorer-drag-drop-feedback: shown when the dragged note
/// would land on itself or one of its descendants. A red ring + "no-drop"
/// cursor signal that the drop will be rejected; releasing here is a no-op.
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

/// Load an artifact's body via the in-scope persistence context,
/// patch its frontmatter `status` to `next`, save back to disk, and
/// bump `LOCAL_NOTE_VERSION` so explorer rows / open tabs re-render.
///
/// Used by the inline row-action icons (Approve / Reject / Mark
/// dirty) so the user can flip an artifact's status without opening
/// it in a tab. Errors are logged via `tracing::warn!`; the row
/// stays put visually if the load or save fails (status dot reverts
/// on the next render because the body wasn't updated).
fn apply_row_status(
    note_id: Uuid,
    next: crate::plugins::artifact::frontmatter::ArtifactStatus,
) {
    let persistence: std::sync::Arc<dyn crate::persistence::Persistence> = use_context();
    let id_str = note_id.to_string();
    dioxus::core::spawn_forever(async move {
        let bytes = match persistence.load(&id_str).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    target: "operon::explorer",
                    "apply_row_status: load({id_str}) failed: {e}"
                );
                return;
            }
        };
        let content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return,
        };
        let new_body = crate::plugins::artifact::view::patch_status_text(&content, next);
        if let Err(e) = persistence.save(&id_str, new_body.as_bytes()).await {
            tracing::warn!(
                target: "operon::explorer",
                "apply_row_status: save({id_str}) failed: {e}"
            );
            return;
        }
        crate::shell::companion_state::LOCAL_NOTE_VERSION
            .with_mut(|v| *v = v.saturating_add(1));
    });
}

/// Begin a Revise session from the explorer row: snapshot the
/// artifact's current body into `ROW_REVISE_SNAPSHOTS` so a later
/// Cancel can revert, then flip the tab to Edit. We snapshot from
/// disk because the row's Revise button is only reachable when the
/// tab is NOT in Edit mode — disk is authoritative there. If a tab
/// is already open in View / Split, the buffer matches disk
/// (manual-save tabs only diverge after the user types in Edit).
///
/// `on_set_mode` is invoked synchronously *after* the snapshot is
/// stored so the renderer never sees Edit-mode with an empty
/// snapshot entry.
fn enter_row_revise(
    note_id: Uuid,
    on_set_mode: dioxus::prelude::Callback<(Uuid, crate::editor::EditorMode)>,
) {
    let persistence: std::sync::Arc<dyn crate::persistence::Persistence> = use_context();
    let id_str = note_id.to_string();
    dioxus::core::spawn_forever(async move {
        let bytes = match persistence.load(&id_str).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    target: "operon::explorer",
                    "enter_row_revise: load({id_str}) failed: {e}"
                );
                return;
            }
        };
        let content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return,
        };
        crate::shell::companion_state::ROW_REVISE_SNAPSHOTS.with_mut(|m| {
            m.insert(note_id, content);
        });
        on_set_mode.call((note_id, crate::editor::EditorMode::Edit));
    });
}

/// Commit a Revise session started from the row: append a revision
/// row to the current tab buffer, save, mark Approved descendants
/// dirty, flip back to View, clear the snapshot. Uses the in-buffer
/// body (not disk) so any in-flight edits the user made during Edit
/// mode are captured — manual-save artifact tabs hold pending edits
/// in `TabManager` until an explicit save.
fn apply_row_revise_done(note_id: Uuid) {
    let persistence: std::sync::Arc<dyn crate::persistence::Persistence> = use_context();
    let crate::local_mode::desktop::LocalNoteRepo(note_repo) = use_context();
    let mut tabs: dioxus::prelude::Signal<crate::tabs::TabManager> = use_context();
    let id_str = note_id.to_string();
    // Resolve the tab + current buffer for this artifact. If no tab
    // is open (shouldn't happen — Edit mode requires an open tab —
    // but defensive) fall back to disk.
    let tab_snapshot: Option<(crate::tabs::TabId, String)> = {
        let snap = tabs.read();
        let found = snap
            .iter()
            .find(|t| t.note_id == id_str)
            .map(|t| (t.id, t.content.clone()));
        found
    };
    dioxus::core::spawn_forever(async move {
        let body_now = match &tab_snapshot {
            Some((_, content)) => content.clone(),
            None => match persistence.load(&id_str).await {
                Ok(b) => String::from_utf8(b).unwrap_or_default(),
                Err(e) => {
                    tracing::warn!(
                        target: "operon::explorer",
                        "apply_row_revise_done: load({id_str}) failed: {e}"
                    );
                    return;
                }
            },
        };
        let prior = crate::shell::companion_state::ROW_REVISE_SNAPSHOTS
            .with_mut(|m| m.remove(&note_id));
        let summary = crate::plugins::artifact::revision_table::compute_summary(
            prior.as_deref(),
            Some(body_now.as_str()),
        );
        let date = crate::plugins::artifact::revision_table::format_revision_date(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        );
        let row = crate::plugins::artifact::revision_table::RevisionRow {
            revision: crate::plugins::artifact::revision_table::next_revision_number(&body_now),
            date,
            derived_from: "manual".to_string(),
            summary,
        };
        let body_with_row =
            crate::plugins::artifact::revision_table::append_revision_row(&body_now, row);

        if let Err(e) = persistence
            .save(&id_str, body_with_row.as_bytes())
            .await
        {
            tracing::warn!(
                target: "operon::explorer",
                "apply_row_revise_done: save({id_str}) failed: {e}"
            );
            return;
        }
        if let Some((tab_id, _)) = tab_snapshot {
            tabs.write().reload_content(tab_id, body_with_row.clone());
            tabs.write()
                .set_mode(tab_id, crate::editor::EditorMode::View);
        }
        match crate::plugins::artifact::view::mark_descendants_dirty(
            &note_repo,
            &persistence,
            note_id,
        )
        .await
        {
            Ok(n) => tracing::info!(
                target: "operon::explorer",
                "apply_row_revise_done: marked {n} descendant(s) of {note_id} dirty"
            ),
            Err(e) => tracing::warn!(
                target: "operon::explorer",
                "apply_row_revise_done: mark_descendants_dirty({note_id}): {e}"
            ),
        }
        crate::shell::companion_state::LOCAL_NOTE_VERSION
            .with_mut(|v| *v = v.saturating_add(1));
    });
}

/// Cancel a Revise session started from the row: revert disk + tab
/// buffer to the snapshot taken at Revise click, flip back to View,
/// clear the snapshot. If no snapshot exists (e.g. user entered
/// Edit through some other path) we still flip back to View without
/// touching disk so the action stays predictable.
fn apply_row_revise_cancel(note_id: Uuid) {
    let persistence: std::sync::Arc<dyn crate::persistence::Persistence> = use_context();
    let mut tabs: dioxus::prelude::Signal<crate::tabs::TabManager> = use_context();
    let id_str = note_id.to_string();
    let tab_id: Option<crate::tabs::TabId> = {
        let snap = tabs.read();
        let found = snap.iter().find(|t| t.note_id == id_str).map(|t| t.id);
        found
    };
    let prior = crate::shell::companion_state::ROW_REVISE_SNAPSHOTS
        .with_mut(|m| m.remove(&note_id));
    if let Some(tid) = tab_id {
        if let Some(ref body) = prior {
            tabs.write().reload_content(tid, body.clone());
        }
        tabs.write().set_mode(tid, crate::editor::EditorMode::View);
    }
    if let Some(body) = prior {
        dioxus::core::spawn_forever(async move {
            if let Err(e) = persistence.save(&id_str, body.as_bytes()).await {
                tracing::warn!(
                    target: "operon::explorer",
                    "apply_row_revise_cancel: revert save({id_str}) failed: {e}"
                );
                return;
            }
            crate::shell::companion_state::LOCAL_NOTE_VERSION
                .with_mut(|v| *v = v.saturating_add(1));
        });
    }
}
