//! Main area: hosts the tab strip, mode toolbar, and renders the active tab via its
//! `FormatPlugin` according to the tab's current `EditorMode`.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::editor::EditorMode;
use crate::plugin::PluginRegistry;
use crate::rbag::state::{AppState, Mode};
use crate::shell::mode_toolbar::ModeToolbar;
use crate::tabs::{SaveScheduler, TabId, TabManager, TabStrip};

#[component]
pub fn MainArea() -> Element {
    let tabs: Signal<TabManager> = use_context();
    let registry: Rc<PluginRegistry> = use_context();
    let scheduler: SaveScheduler = use_context();
    let app_state: Signal<AppState> = use_context();
    let is_local = app_state.read().mode == Mode::Local;

    let active_info: Option<(TabId, String, String, String, EditorMode)> = {
        let snapshot = tabs.read();
        snapshot.active().map(|tab| {
            (
                tab.id,
                tab.format_id.clone(),
                tab.note_id.clone(),
                tab.content.clone(),
                tab.mode,
            )
        })
    };

    let body: Element = match active_info {
        None => rsx! {
            div { class: "operon-main-empty",
                "No notes open — open one from the side bar or via the command palette."
            }
        },
        Some((tab_id, format_id, note_id, content, mode)) => {
            match registry.format_plugin_for(&format_id) {
                Some(plugin) => {
                    // on_change writes the new content back through the TabManager and
                    // schedules a debounced save through Persistence. The scheduler clears
                    // dirty on success. Signal<TabManager> is Copy, so we hand a fresh
                    // copy into each closure that needs to call .write().
                    let scheduler_for_change = scheduler.clone();
                    let note_id_for_change = note_id.clone();
                    let tabs_handle = tabs;
                    let on_change = EventHandler::new(move |new_content: String| {
                        let mut t = tabs_handle;
                        t.write().set_content(tab_id, new_content.clone());
                        scheduler_for_change.schedule(
                            tab_id,
                            note_id_for_change.clone(),
                            new_content,
                            move || {
                                let mut t = tabs_handle;
                                t.write().set_dirty(tab_id, false);
                            },
                        );
                    });
                    let local_save: Option<crate::local_mode::LocalSaveAction> =
                        try_consume_context();
                    match mode {
                        EditorMode::View => plugin.render(&note_id, &content),
                        EditorMode::Edit => {
                            // Local Mode dispatch:
                            //   - Markdown keeps the LocalNoteEditor shell
                            //     (Monaco + paste-image + wikilink picker).
                            //   - Every other format_id (image, mdx, code,
                            //     kanban, canvas, excalidraw, …) goes
                            //     through its FormatPlugin's render_edit
                            //     so each kind gets its own bespoke editor
                            //     surface.
                            //   - If somehow a kind without a registered
                            //     plugin slips through, we fall back to
                            //     LocalNoteEditor so the user can still
                            //     edit text.
                            // `key` forces Dioxus to unmount the prior
                            // editor (and any embedded Monaco bootstrap)
                            // when the active tab changes, then mount a
                            // fresh instance for the new tab.
                            if is_local {
                                let uses_local_shell = format_id.as_str() == "markdown";
                                if uses_local_shell {
                                    if let Some(action) = local_save {
                                        rsx! {
                                            crate::local_mode::LocalNoteEditor {
                                                key: "{tab_id:?}",
                                                tab_id,
                                                action,
                                            }
                                        }
                                    } else {
                                        plugin.render_edit(&note_id, &content, on_change)
                                    }
                                } else {
                                    plugin.render_edit(&note_id, &content, on_change)
                                }
                            } else {
                                plugin.render_edit(&note_id, &content, on_change)
                            }
                        }
                        EditorMode::LivePreview => {
                            plugin.render_live_preview(&note_id, &content, on_change)
                        }
                        EditorMode::Split => {
                            // Local Split: side-by-side textarea + rendered view.
                            if is_local {
                                if let Some(action) = local_save {
                                    rsx! {
                                        div {
                                            class: "operon-local-split",
                                            div {
                                                class: "operon-local-split-edit",
                                                crate::local_mode::LocalNoteEditor {
                                                    key: "{tab_id:?}",
                                                    tab_id,
                                                    action,
                                                }
                                            }
                                            div {
                                                class: "operon-local-split-view",
                                                {plugin.render(&note_id, &content)}
                                            }
                                        }
                                    }
                                } else {
                                    rsx! {
                                        crate::shell::split_view::SplitView {
                                            format_id: format_id.clone(),
                                            note_id: note_id.clone(),
                                            content: content.clone(),
                                            on_change,
                                        }
                                    }
                                }
                            } else {
                                rsx! {
                                    crate::shell::split_view::SplitView {
                                        format_id: format_id.clone(),
                                        note_id: note_id.clone(),
                                        content: content.clone(),
                                        on_change,
                                    }
                                }
                            }
                        }
                    }
                }
                None => rsx! {
                    div { class: "operon-main-empty",
                        "No plugin registered for format {format_id:?}"
                    }
                },
            }
        }
    };

    rsx! {
        section {
            "data-region": "main-area",
            class: "operon-region operon-main-area",
            TabStrip {}
            // Local Mode hides the View/Edit/Live Preview/Split toolbar — mode
            // switching happens via the note row's right-click context menu.
            if !is_local { ModeToolbar {} }
            div { class: "operon-main-body", {body} }
        }
    }
}
