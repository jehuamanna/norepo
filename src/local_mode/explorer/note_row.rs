//! A single note row inside the explorer panel. Indented by `depth * 16px`,
//! shows a disclosure caret only when it has children, and supports inline
//! rename / context menu (Rename / Delete + disabled placeholders).

use dioxus::prelude::*;
use operon_store::repos::LocalNote;
use uuid::Uuid;

use crate::local_mode::ui::{ContextMenu, ContextMenuItem, InlineRename};

#[derive(Props, Clone, PartialEq)]
pub struct NoteRowProps {
    pub note: LocalNote,
    pub depth: i64,
    pub has_children: bool,
    pub is_open: bool,
    pub selected: bool,
    pub in_rename: bool,
    pub on_select: Callback<Uuid>,
    pub on_toggle_open: Callback<Uuid>,
    pub on_rename: Callback<(Uuid, String)>,
    pub on_request_rename: Callback<Uuid>,
    pub on_request_delete: Callback<Uuid>,
    pub on_add_child: Callback<Uuid>,
}

#[component]
pub fn NoteRow(props: NoteRowProps) -> Element {
    let menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut menu_pos_setter = menu_pos;

    let note = props.note.clone();
    let id = note.id;
    let id_str = id.to_string();
    let title = note.title.clone();
    let depth = props.depth.max(0);
    let indent_px = depth * 16;
    let has_children = props.has_children;
    let is_open = props.is_open;
    let selected = props.selected;
    let in_rename = props.in_rename;

    let on_select = props.on_select;
    let on_toggle_open = props.on_toggle_open;
    let on_rename = props.on_rename;
    let on_request_rename = props.on_request_rename;
    let on_request_delete = props.on_request_delete;
    let on_add_child = props.on_add_child;

    let row_class = if selected {
        "flex items-center gap-1 px-2 py-1 cursor-pointer text-sm bg-[var(--operon-hover)]"
    } else {
        "flex items-center gap-1 px-2 py-1 cursor-pointer text-sm hover:bg-[var(--operon-hover)] group"
    };
    let style = format!("padding-left: {}px;", 8 + indent_px);

    let initial_title = title.clone();
    let dismiss_menu = use_callback(move |_: ()| menu_pos_setter.set(None));

    let menu_items: Vec<ContextMenuItem> = vec![
        ContextMenuItem::new(
            "Rename",
            Callback::new(move |_| {
                on_request_rename.call(id);
            }),
        ),
        ContextMenuItem::new(
            "Add child note",
            Callback::new(move |_| {
                on_add_child.call(id);
            }),
        ),
        ContextMenuItem::new(
            "Delete",
            Callback::new(move |_| {
                on_request_delete.call(id);
            }),
        ),
        ContextMenuItem::disabled("Cut"),
        ContextMenuItem::disabled("Copy"),
        ContextMenuItem::disabled("Paste"),
    ];

    let caret_glyph = if has_children {
        if is_open {
            "\u{25BE}"
        } else {
            "\u{25B8}"
        }
    } else {
        ""
    };

    rsx! {
        div {
            class: "{row_class}",
            style: "{style}",
            "data-testid": "note-row",
            "data-note-id": "{id_str}",
            "data-note-depth": "{depth}",
            "data-selected": if selected { "true" } else { "false" },
            "data-open": if is_open { "true" } else { "false" },
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
            // Disclosure caret — only visible when has_children, but we render
            // a fixed-width slot either way so labels line up.
            span {
                class: "inline-flex w-3 shrink-0 select-none text-xs opacity-70",
                "data-testid": "note-row-disclosure",
                "data-has-children": if has_children { "true" } else { "false" },
                onclick: move |evt| {
                    evt.stop_propagation();
                    if has_children {
                        on_toggle_open.call(id);
                    }
                },
                "{caret_glyph}"
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
