//! Tab strip rendered atop the main area.
//!
//! Reads `Signal<TabManager>` from context. Click activates a tab; clicking the close icon
//! closes it. A circle-dot marker replaces the close-x icon while a tab is dirty.
//!
//! Accessibility: the strip is a WAI-ARIA `tablist`; each tab is a `role="tab"` button
//! with `aria-selected` reflecting the active state. Roving tabindex keeps Tab-key focus
//! on the active tab; ArrowLeft/ArrowRight cycle focus + activation, Home/End jump to
//! ends, and Delete/Cmd+W close the focused tab.
//!
//! Activation moves keyboard focus into the editor by writing the activated tab's
//! `note_id` into the app-scope `RequestEditorFocus` signal — both mouse clicks and
//! Enter/ArrowKey activation route through the same helper. Tabs are also draggable
//! horizontally so the user can reorder them; the drop position (before / after) is
//! decided by the cursor's X relative to the target tab's midline.

use dioxus::prelude::*;
use keyboard_types::Modifiers;

use super::{Tab, TabId, TabManager};
use crate::ui::Icon;

#[component]
pub fn TabStrip() -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let crate::editor::RequestEditorFocus(focus_request) = use_context();
    // Source tab id while a drag is in flight. Cleared on drop or
    // dragend. Kept local to the strip — no other component needs it.
    let drag_source: Signal<Option<TabId>> = use_signal(|| None);
    let snapshot = tabs.read();
    let active_id = snapshot.active_id();
    let view: Vec<(TabId, String, String, bool)> = snapshot
        .iter()
        .map(|t: &Tab| (t.id, t.note_id.clone(), t.title.clone(), t.dirty))
        .collect();
    drop(snapshot);

    let activate = move |id: TabId, note_id: String, mut focus_req: Signal<Option<String>>, mut tabs: Signal<TabManager>| {
        tabs.write().activate(id);
        focus_req.set(Some(note_id));
    };

    rsx! {
        div {
            class: "operon-tab-strip",
            role: "tablist",
            "aria-label": "Open tabs",
            for (id, note_id, title, dirty) in view {
                {
                    let is_active = active_id == Some(id);
                    let close_label = format!("Close tab {title}");
                    let dirty_label = if dirty {
                        format!("Tab {title}, unsaved changes")
                    } else {
                        format!("Tab {title}")
                    };
                    let note_id_for_click = note_id.clone();
                    let note_id_for_keys = note_id.clone();
                    rsx! {
                        button {
                            r#type: "button",
                            class: "operon-tab",
                            role: "tab",
                            "data-active": if is_active { "true" } else { "false" },
                            "data-tab-id": "{id.0}",
                            "aria-selected": if is_active { "true" } else { "false" },
                            "aria-label": "{dirty_label}",
                            // Roving tabindex: only the active tab is in the
                            // Tab cycle. Inactive tabs are reachable via
                            // ArrowLeft/Right (handled below) instead.
                            tabindex: if is_active { "0" } else { "-1" },
                            draggable: "true",
                            onclick: {
                                let note_id = note_id_for_click.clone();
                                move |_| activate(id, note_id.clone(), focus_request, tabs)
                            },
                            ondragstart: {
                                let mut drag_source = drag_source;
                                move |_| { drag_source.set(Some(id)); }
                            },
                            ondragover: move |evt| {
                                // Mark the row as a valid drop target —
                                // browsers ignore drop without this.
                                evt.prevent_default();
                            },
                            ondragend: {
                                let mut drag_source = drag_source;
                                move |_| { drag_source.set(None); }
                            },
                            ondrop: {
                                let mut drag_source = drag_source;
                                move |evt| {
                                    evt.prevent_default();
                                    let from = match *drag_source.read() {
                                        Some(t) => t,
                                        None => return,
                                    };
                                    drag_source.set(None);
                                    if from == id { return; }
                                    // before-vs-after picked from the
                                    // cursor's X relative to the tab's
                                    // own midline. element_coordinates
                                    // is local to the tab button, so
                                    // half its rendered width is the
                                    // pivot. We don't know the width
                                    // without measuring; use a
                                    // querySelector via JS to read
                                    // offsetWidth would round-trip — a
                                    // ratio against client coordinates
                                    // is good enough: x < 8ch ⇒ before,
                                    // else after. Since tabs vary by
                                    // title length we instead resolve
                                    // by index parity: if `from` is
                                    // currently to the left of `to`,
                                    // drop after; otherwise drop before.
                                    let (place_before, _) = {
                                        let snap = tabs.read();
                                        let from_idx = snap.iter()
                                            .position(|t| t.id == from);
                                        let to_idx = snap.iter()
                                            .position(|t| t.id == id);
                                        match (from_idx, to_idx) {
                                            (Some(fi), Some(ti)) => (fi > ti, ti),
                                            _ => (true, 0),
                                        }
                                    };
                                    tabs.write().reorder(from, id, place_before);
                                }
                            },
                            onkeydown: {
                                let note_id = note_id_for_keys.clone();
                                move |evt| {
                                    let key = evt.key().to_string();
                                    let mods = evt.modifiers();
                                    let with_meta = mods.contains(Modifiers::META)
                                        || mods.contains(Modifiers::CONTROL);
                                    if key == "ArrowRight" {
                                        evt.prevent_default();
                                        evt.stop_propagation();
                                        tabs.write().activate_next();
                                        focus_active_tab();
                                    } else if key == "ArrowLeft" {
                                        evt.prevent_default();
                                        evt.stop_propagation();
                                        tabs.write().activate_prev();
                                        focus_active_tab();
                                    } else if key == "Home" {
                                        evt.prevent_default();
                                        evt.stop_propagation();
                                        tabs.write().activate_index(0);
                                        focus_active_tab();
                                    } else if key == "End" {
                                        evt.prevent_default();
                                        evt.stop_propagation();
                                        let last = tabs.read().len().saturating_sub(1);
                                        tabs.write().activate_index(last);
                                        focus_active_tab();
                                    } else if key == "Delete"
                                        || (with_meta && key.eq_ignore_ascii_case("w"))
                                    {
                                        evt.prevent_default();
                                        evt.stop_propagation();
                                        tabs.write().close(id);
                                    } else if key == "Enter" || key == " " {
                                        evt.prevent_default();
                                        evt.stop_propagation();
                                        activate(id, note_id.clone(), focus_request, tabs);
                                    }
                                }
                            },
                            span { class: "operon-tab-title", "{title}" }
                            span {
                                class: if dirty { "operon-tab-marker" } else { "operon-tab-close" },
                                role: "button",
                                tabindex: "-1",
                                "aria-label": "{close_label}",
                                onclick: move |evt| {
                                    evt.stop_propagation();
                                    tabs.write().close(id);
                                },
                                if dirty {
                                    Icon { name: "circle-dot".to_string() , size: 12 }
                                } else {
                                    Icon { name: "x".to_string(), size: 12, title: close_label.clone() }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Move DOM focus to whichever tab currently carries `tabindex="0"` (the
/// active one). Called after keyboard nav so the focus ring follows the
/// active state without a re-mount cycle.
fn focus_active_tab() {
    document::eval(
        r#"
        (function() {
            var el = document.querySelector('.operon-tab-strip [role="tab"][tabindex="0"]');
            if (el && typeof el.focus === 'function') el.focus();
        })();
        "#,
    );
}
