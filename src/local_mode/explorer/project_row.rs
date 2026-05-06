//! A single project row inside [`crate::local_mode::explorer::ExplorerPanel`].

use dioxus::prelude::*;
use dioxus::html::HasFileData;
use operon_store::repos::LocalProject;
use uuid::Uuid;

use operon_store::repos::NoteKind;

use crate::local_mode::ui::{
    classify_drop_position, ContextMenu, ContextMenuItem, DragKind, DragSession, DropPosition,
    InlineRename,
};

#[derive(Props, Clone, PartialEq)]
pub struct ProjectRowProps {
    pub project: LocalProject,
    pub is_open: bool,
    pub selected: bool,
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
    /// `Add image note…` item used.
    pub on_add_note: Callback<(Uuid, NoteKind)>,
    /// Plans-Phase-6-image-notes: external image-file drops onto this
    /// project row land as top-level image-notes in the project. Tuple is
    /// (project_id, bytes, suggested filename).
    pub on_drop_image_file: Callback<(Uuid, Vec<u8>, String)>,
    pub on_cut: Callback<Uuid>,
    pub on_copy: Callback<Uuid>,
    pub on_paste: Callback<Uuid>,
    pub on_drop_project_on_project: Callback<(Uuid, Uuid, DropPosition)>,
    pub on_drop_note_on_project: Callback<(Uuid, Uuid, DropPosition)>,
}

#[component]
pub fn ProjectRow(props: ProjectRowProps) -> Element {
    let menu_pos: Signal<Option<(i32, i32)>> = use_signal(|| None);
    let mut menu_pos_setter = menu_pos;
    // Plans-Phase-7-projectrow-forbidden: tri-state indicator mirroring
    // NoteRow. Some(Ok(pos)) → allowed; Some(Err(())) → forbidden
    // (self-drop, or a position not allowed for the dragged kind);
    // None → no drag over this row.
    let drop_indicator: Signal<Option<Result<DropPosition, ()>>> = use_signal(|| None);
    let mut drop_indicator_setter = drop_indicator;
    let DragSession(mut drag_session) = use_context();

    let project = props.project.clone();
    let id = project.id;
    let id_str = id.to_string();
    let name = project.name.clone();

    let selected = props.selected;
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

    let mut row_class = if selected {
        String::from("notes-explorer-row notes-explorer-row-active group")
    } else {
        String::from("notes-explorer-row group")
    };
    if cut {
        row_class.push_str(" notes-explorer-row-cut");
    }
    let style = "--depth: 0;";

    let initial_name = name.clone();
    let dismiss_menu = use_callback(move |_: ()| menu_pos_setter.set(None));

    let mut paste_item = ContextMenuItem::new(
        "Paste",
        Callback::new(move |_| {
            on_paste.call(id);
        }),
    );
    paste_item.enabled = has_clip_note;

    let menu_items: Vec<ContextMenuItem> = vec![
        ContextMenuItem::new(
            "Rename",
            Callback::new(move |_| {
                on_request_rename.call(id);
            }),
        ),
        ContextMenuItem::submenu(
            "Add note",
            vec![
                ContextMenuItem::new(
                    "Markdown",
                    Callback::new(move |_| {
                        on_add_note.call((id, NoteKind::Markdown));
                    }),
                ),
                ContextMenuItem::new(
                    "Image",
                    Callback::new(move |_| {
                        on_add_note.call((id, NoteKind::Image));
                    }),
                ),
            ],
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
        ContextMenuItem::new(
            "Delete",
            Callback::new(move |_| {
                on_request_delete.call(id);
            }),
        ),
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
            draggable: "true",
            onclick: move |evt| {
                evt.stop_propagation();
                on_select.call(id);
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
                let next: Option<Result<DropPosition, ()>> = match kind {
                    Some(DragKind::Project(src)) => {
                        if src == id {
                            Some(Err(()))
                        } else if matches!(pos, DropPosition::Into) {
                            Some(Err(()))
                        } else {
                            Some(Ok(pos))
                        }
                    }
                    Some(DragKind::Note(_)) => {
                        if matches!(pos, DropPosition::Into) {
                            Some(Ok(pos))
                        } else {
                            Some(Err(()))
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
                // Plans-Phase-6-image-notes: external image-file drop creates
                // a top-level image-note in this project.
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
                match kind {
                    Some(DragKind::Project(src)) if src != id && !matches!(pos, DropPosition::Into) => {
                        on_drop_project_on_project.call((src, id, pos));
                    }
                    Some(DragKind::Note(src)) if matches!(pos, DropPosition::Into) => {
                        on_drop_note_on_project.call((src, id, pos));
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
                    class: "truncate flex-1",
                    "data-testid": "project-row-name",
                    "{name}"
                }
                button {
                    r#type: "button",
                    class: "opacity-0 group-hover:opacity-100 inline-flex items-center justify-center w-5 h-5 rounded text-xs hover:bg-[var(--operon-border)]",
                    "data-testid": "add-note-button",
                    "data-project-id": "{id_str}",
                    "aria-label": "Add note",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        // Plans-Phase-1: quick-button defaults to Markdown.
                        on_add_note.call((id, NoteKind::Markdown));
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
