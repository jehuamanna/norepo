//! A single note row inside the explorer panel. Indented by `depth * 16px`,
//! shows a disclosure caret only when it has children, and supports inline
//! rename / context menu (Phase-4: cut/copy/paste, indent/outdent, move-up/down).

use dioxus::prelude::*;
use dioxus::html::HasFileData;
use keyboard_types::Modifiers;
use operon_store::repos::{LocalNote, NoteKind};
use uuid::Uuid;

use crate::editor::EditorMode;
use crate::local_mode::explorer::{
    ExplorerUndoCtx, LastClicked, MultiSelected, NodeKey, NotesByProjectCtx, VisibleFlat,
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
    /// Plans-Phase-1-note-creation-context-menu: kind-aware add-child. The
    /// submenu's Markdown / Image leaves dispatch with the chosen NoteKind;
    /// the handler in `explorer/mod.rs` branches on kind (Markdown takes
    /// the existing fast path, Image opens a file picker before creation).
    pub on_add_child: Callback<(Uuid, NoteKind)>,
    /// Plans-Phase-1-note-creation-context-menu: kind-aware add-sibling.
    /// Same dispatch contract as `on_add_child`. The Markdown branch creates
    /// then `move_to(.., target.sibling_index + 1)`; Image goes via the
    /// project-scoped picker plumbing with the target's `parent_id`.
    pub on_add_sibling: Callback<(Uuid, NoteKind)>,
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

    let note = props.note.clone();
    let id = note.id;
    let id_str = id.to_string();
    let title = note.title.clone();
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

    // Operon-Phase-3: the + dropdown creates a CHILD note (matches the
    // historical default behavior of this button). Items mirror
    // `NoteKind::all_creatable()` so adding a future variant lights up
    // here automatically.
    let add_menu_items: Vec<ContextMenuItem> = NoteKind::all_creatable()
        .iter()
        .copied()
        .map(|kind| {
            let label = kind.display_name();
            ContextMenuItem::new(
                label,
                Callback::new(move |_| {
                    on_add_child.call((id, kind));
                }),
            )
        })
        .collect();

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
            "Edit",
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
            NoteKind::all_creatable()
                .iter()
                .copied()
                .map(|kind| {
                    ContextMenuItem::new(
                        kind.display_name(),
                        Callback::new(move |_| {
                            on_add_child.call((id, kind));
                        }),
                    )
                })
                .collect(),
        ),
        ContextMenuItem::submenu(
            "Add sibling note",
            NoteKind::all_creatable()
                .iter()
                .copied()
                .map(|kind| {
                    ContextMenuItem::new(
                        kind.display_name(),
                        Callback::new(move |_| {
                            on_add_sibling.call((id, kind));
                        }),
                    )
                })
                .collect(),
        ),
        ContextMenuItem::new(
            "Cut",
            Callback::new(move |_| {
                on_cut.call(id);
            }),
        ),
        ContextMenuItem::new(
            "Copy",
            Callback::new(move |_| {
                on_copy.call(id);
            }),
        ),
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
        ContextMenuItem::new(
            "Delete",
            Callback::new(move |_| {
                on_request_delete.call(id);
            }),
        ),
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
                on_select.call(id);
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
                    if key == "Tab" {
                        evt.prevent_default();
                        evt.stop_propagation();
                        if mods.contains(Modifiers::SHIFT) {
                            on_outdent.call(id);
                        } else {
                            on_indent.call(id);
                        }
                    } else if key == "ArrowUp" && mods.contains(Modifiers::ALT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_move_up.call(id);
                    } else if key == "ArrowDown" && mods.contains(Modifiers::ALT) {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_move_down.call(id);
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
                            Some(Ok(pos))
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
                // Plans-Phase-6-image-notes: external file drop. If the
                // event carries any FileData we treat the drop as image
                // imports (creating child image-notes under this row) and
                // ignore the in-app DragKind path for this event.
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
                    drag_session.set(None);
                    drop_indicator_setter.set(None);
                    return;
                }
                let kind = *drag_session.read();
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
                    // target. Otherwise just the source.
                    let set_snap = multi_selected.read().clone();
                    let multi_drop = set_snap.contains(&NodeKey::Note(src));
                    if multi_drop {
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
                {
                    let icon = note.kind.icon();
                    let kind_str = note.kind.as_str();
                    rsx! {
                        span {
                            class: "kind-badge kind-{kind_str} text-[0.65rem] mr-1 px-1 rounded select-none opacity-60 shrink-0",
                            "data-testid": "kind-badge",
                            "data-note-kind": "{kind_str}",
                            "[{icon}]"
                        }
                    }
                }
                span {
                    class: "truncate flex-1",
                    "data-testid": "note-row-name",
                    "{title}"
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
