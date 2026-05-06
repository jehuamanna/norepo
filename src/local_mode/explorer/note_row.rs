//! A single note row inside the explorer panel. Indented by `depth * 16px`,
//! shows a disclosure caret only when it has children, and supports inline
//! rename / context menu (Phase-4: cut/copy/paste, indent/outdent, move-up/down).

use dioxus::prelude::*;
use keyboard_types::Modifiers;
use operon_store::repos::LocalNote;
use uuid::Uuid;

use crate::editor::EditorMode;
use crate::local_mode::ui::{
    classify_drop_position, ContextMenu, ContextMenuItem, DragKind, DragSession, DropPosition,
    InlineRename,
};

#[derive(Props, Clone, PartialEq)]
pub struct NoteRowProps {
    pub note: LocalNote,
    pub depth: i64,
    pub has_children: bool,
    pub is_open: bool,
    pub selected: bool,
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
    pub on_add_child: Callback<Uuid>,
    /// Plans-Phase-3-note-id-create: insert a new sibling note immediately
    /// after this one, at the same depth. The handler in `explorer/mod.rs`
    /// finds the target's parent via `notes_by_project`, calls
    /// `note_repo.create(.., parent_id, "")`, then `move_to` to land at
    /// `target.sibling_index + 1` and triggers the inline rename.
    pub on_add_sibling: Callback<Uuid>,
    pub on_indent: Callback<Uuid>,
    pub on_outdent: Callback<Uuid>,
    pub on_move_up: Callback<Uuid>,
    pub on_move_down: Callback<Uuid>,
    pub on_cut: Callback<Uuid>,
    pub on_copy: Callback<Uuid>,
    pub on_paste: Callback<Uuid>,
    pub on_drop_note_on_note: Callback<(Uuid, Uuid, DropPosition)>,
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
    let drop_indicator: Signal<Option<DropPosition>> = use_signal(|| None);
    let mut drop_indicator_setter = drop_indicator;
    let DragSession(mut drag_session) = use_context();

    let note = props.note.clone();
    let id = note.id;
    let id_str = id.to_string();
    let title = note.title.clone();
    let depth = props.depth.max(0);
    let has_children = props.has_children;
    let is_open = props.is_open;
    let selected = props.selected;
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
    let current_mode = props.current_mode;
    let on_set_mode = props.on_set_mode;

    let mut row_class = if selected {
        String::from("notes-explorer-row notes-explorer-row-active group")
    } else {
        String::from("notes-explorer-row group")
    };
    if cut {
        row_class.push_str(" notes-explorer-row-cut");
    }
    let style = format!("--depth: {depth};");

    let initial_title = title.clone();
    let dismiss_menu = use_callback(move |_: ()| menu_pos_setter.set(None));

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
        ContextMenuItem::new(
            "Add child note",
            Callback::new(move |_| {
                on_add_child.call(id);
            }),
        ),
        ContextMenuItem::new(
            "Add sibling note",
            Callback::new(move |_| {
                on_add_sibling.call(id);
            }),
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
            },
            ondragend: move |_| {
                drag_session.set(None);
                drop_indicator_setter.set(None);
            },
            ondragover: move |evt| {
                evt.prevent_default();
                let kind = *drag_session.read();
                let coords = evt.element_coordinates();
                let pos = classify_drop_position(coords.y, 28.0);
                let allow = matches!(kind, Some(DragKind::Note(src)) if src != id);
                drop_indicator_setter.set(if allow { Some(pos) } else { None });
            },
            ondragleave: move |_| {
                drop_indicator_setter.set(None);
            },
            ondrop: move |evt| {
                evt.prevent_default();
                let kind = *drag_session.read();
                let coords = evt.element_coordinates();
                let pos = classify_drop_position(coords.y, 28.0);
                if let Some(DragKind::Note(src)) = kind {
                    if src != id {
                        on_drop_note_on_note.call((src, id, pos));
                    }
                }
                drag_session.set(None);
                drop_indicator_setter.set(None);
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
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_add_child.call(id);
                    },
                    "+"
                }
            }
            if drag_active {
                if let Some(p) = drop_pos_now {
                    DropIndicator { position: p }
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
