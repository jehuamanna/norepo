//! A single project row inside [`crate::local_mode::explorer::ExplorerPanel`].

use dioxus::prelude::*;
use operon_store::repos::LocalProject;
use uuid::Uuid;

use crate::local_mode::ui::{ContextMenu, ContextMenuItem, InlineRename};

#[derive(Props, Clone, PartialEq)]
pub struct ProjectRowProps {
    pub project: LocalProject,
    pub selected: bool,
    pub in_rename: bool,
    pub on_select: Callback<Uuid>,
    pub on_rename: Callback<(Uuid, String)>,
    pub on_delete: Callback<Uuid>,
    pub on_request_rename: Callback<Uuid>,
    pub on_request_delete: Callback<Uuid>,
}

#[component]
pub fn ProjectRow(props: ProjectRowProps) -> Element {
    let menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut menu_pos_setter = menu_pos;

    let project = props.project.clone();
    let id = project.id;
    let id_str = id.to_string();
    let name = project.name.clone();

    let selected = props.selected;
    let in_rename = props.in_rename;

    let on_select = props.on_select;
    let on_rename = props.on_rename;
    // `on_delete` is part of the public row API (Phase-3 will use it for keyboard
    // shortcuts that bypass the confirm dialog); Phase-2 routes deletes through
    // `on_request_delete` only.
    let _on_delete = props.on_delete;
    let on_request_rename = props.on_request_rename;
    let on_request_delete = props.on_request_delete;

    let row_class = if selected {
        "flex items-center gap-2 px-2 py-1 cursor-pointer text-sm bg-[var(--operon-hover)]"
    } else {
        "flex items-center gap-2 px-2 py-1 cursor-pointer text-sm hover:bg-[var(--operon-hover)]"
    };

    let initial_name = name.clone();
    let dismiss_menu = use_callback(move |_: ()| menu_pos_setter.set(None));

    let menu_items: Vec<ContextMenuItem> = vec![
        ContextMenuItem::new(
            "Rename",
            Callback::new(move |_| {
                on_request_rename.call(id);
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
        ContextMenuItem::disabled("Drag"),
    ];

    rsx! {
        div {
            class: "{row_class}",
            "data-testid": "project-row",
            "data-project-id": "{id_str}",
            "data-selected": if selected { "true" } else { "false" },
            onclick: move |evt| {
                evt.stop_propagation();
                on_select.call(id);
            },
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                let coords = evt.client_coordinates();
                menu_pos_setter.set(Some((coords.x as i32, coords.y as i32)));
            },
            if in_rename {
                InlineRename {
                    initial: initial_name.clone(),
                    on_commit: Callback::new(move |new_name: String| {
                        on_rename.call((id, new_name));
                    }),
                    on_cancel: Callback::new(move |_| {
                        // Cancel = commit unchanged (rename validates trim, so no DB write
                        // happens for whitespace-only input). Use a sentinel by passing the
                        // original; the caller treats empty-list-update as exit-rename.
                        on_rename.call((id, String::new()));
                    }),
                }
            } else {
                span {
                    class: "truncate flex-1",
                    "data-testid": "project-row-name",
                    "{name}"
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
