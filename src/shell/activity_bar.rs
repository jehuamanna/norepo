//! Activity bar: pinned vertical icon column on the left edge.
//!
//! Iterates `UIPlugin` contributions for [`PluginSurface::ActivityBar`] and renders each as a
//! clickable icon. A bottom-pinned button toggles the side bar's collapse flag on
//! [`crate::shell::layout::LayoutState`].

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::{PluginRegistry, PluginSurface};
use crate::rbag::state::{AppState, Mode};
use crate::shell::layout::LayoutState;
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};
use crate::ui::Icon;

#[component]
pub fn ActivityBar() -> Element {
    let registry: Rc<PluginRegistry> = use_context();
    let ActiveActivity(mut active) = use_context();
    let LastActiveActivity(mut last) = use_context();
    let mut layout: Signal<LayoutState> = use_context();
    let app_state: Signal<AppState> = use_context();
    let settings_open: Option<crate::local_mode::SettingsOpen> = try_consume_context();
    let is_local = app_state.read().mode == Mode::Local;

    let active_id = active.read().clone();
    let collapsed = layout.read().sidebar_collapsed;
    let toggle_icon = if collapsed { "chevron-right" } else { "square" };

    let registry_for_toggle = registry.clone();

    let items: Vec<(ActivityItemId, String, bool, Element)> = registry
        .contributions(PluginSurface::ActivityBar)
        .map(|plugin| {
            let aid = ActivityItemId(format!("{}:default", plugin.manifest().id));
            let aid_str = aid.0.clone();
            let is_active = active_id.as_ref() == Some(&aid);
            let rendered = plugin.render(PluginSurface::ActivityBar);
            (aid, aid_str, is_active, rendered)
        })
        .collect();

    rsx! {
        section {
            "data-region": "activity-bar",
            class: "operon-region operon-activity-bar",
            for (aid, aid_str, is_active, rendered) in items {
                div {
                    class: if is_active { "operon-activity-item operon-activity-item-active" } else { "operon-activity-item" },
                    "data-activity-id": "{aid_str}",
                    onclick: move |_| {
                        let cur = active.read().clone();
                        if cur.as_ref() == Some(&aid) {
                            last.set(cur);
                            active.set(None);
                        } else {
                            active.set(Some(aid.clone()));
                        }
                    },
                    {rendered}
                }
            }
            div { style: "flex: 1 1 auto;" }
            if let (true, Some(so_ctx)) = (is_local, settings_open) {
                button {
                    class: "operon-activity-toggle",
                    "data-testid": "settings-gear",
                    title: "Settings",
                    onclick: {
                        let mut so = so_ctx.0;
                        move |_| { so.set(true); }
                    },
                    Icon { name: "settings".to_string() }
                }
            }
            button {
                class: "operon-activity-toggle",
                "data-action": "toggle-sidebar",
                title: "Toggle Side Bar",
                onclick: move |_| {
                    layout.with_mut(|s| s.toggle_sidebar());
                    if active.read().is_none() {
                        let next = last.read().clone().or_else(|| {
                            registry_for_toggle
                                .contributions(PluginSurface::ActivityBar)
                                .next()
                                .map(|p| ActivityItemId(format!("{}:default", p.manifest().id)))
                        });
                        if let Some(id) = next {
                            active.set(Some(id));
                        }
                    }
                },
                Icon { name: toggle_icon.to_string() }
            }
        }
    }
}
