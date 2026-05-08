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
    let toggle_label = if collapsed {
        "Show side bar"
    } else {
        "Hide side bar"
    };

    let registry_for_toggle = registry.clone();

    let items: Vec<(ActivityItemId, String, bool, String, Element)> = registry
        .contributions(PluginSurface::ActivityBar)
        .map(|plugin| {
            let aid = ActivityItemId(format!("{}:default", plugin.manifest().id));
            let aid_str = aid.0.clone();
            let is_active = active_id.as_ref() == Some(&aid);
            let label = plugin.manifest().display_name.clone();
            let rendered = plugin.render(PluginSurface::ActivityBar);
            (aid, aid_str, is_active, label, rendered)
        })
        .collect();

    rsx! {
        section {
            "data-region": "activity-bar",
            class: "operon-region operon-activity-bar",
            role: "tablist",
            "aria-label": "Activity bar",
            "aria-orientation": "vertical",
            for (aid, aid_str, is_active, label, rendered) in items {
                {
                    let aid_for_click = aid.clone();
                    let aid_for_keys = aid.clone();
                    rsx! {
                        div {
                            class: if is_active { "operon-activity-item operon-activity-item-active" } else { "operon-activity-item" },
                            "data-activity-id": "{aid_str}",
                            role: "tab",
                            "aria-label": "{label}",
                            "aria-selected": if is_active { "true" } else { "false" },
                            tabindex: if is_active { "0" } else { "-1" },
                            title: "{label}",
                            onclick: move |_| {
                                let cur = active.read().clone();
                                if cur.as_ref() == Some(&aid_for_click) {
                                    last.set(cur);
                                    active.set(None);
                                } else {
                                    active.set(Some(aid_for_click.clone()));
                                }
                            },
                            onkeydown: move |evt| {
                                let key = evt.key().to_string();
                                if key == "Enter" || key == " " {
                                    evt.prevent_default();
                                    let cur = active.read().clone();
                                    if cur.as_ref() == Some(&aid_for_keys) {
                                        last.set(cur);
                                        active.set(None);
                                    } else {
                                        active.set(Some(aid_for_keys.clone()));
                                    }
                                } else if key == "ArrowDown" || key == "ArrowUp" {
                                    evt.prevent_default();
                                    let dir = if key == "ArrowDown" { 1i32 } else { -1i32 };
                                    let script = format!(
                                        r#"
                                        (function() {{
                                            var nodes = Array.prototype.slice.call(document.querySelectorAll('[data-region="activity-bar"] [data-activity-id]'));
                                            if (!nodes.length) return;
                                            var cur = document.activeElement;
                                            var idx = nodes.indexOf(cur);
                                            if (idx < 0) idx = 0;
                                            var next = idx + ({dir});
                                            if (next < 0) next = nodes.length - 1;
                                            if (next >= nodes.length) next = 0;
                                            nodes[next].focus();
                                        }})();
                                        "#
                                    );
                                    document::eval(&script);
                                }
                            },
                            {rendered}
                        }
                    }
                }
            }
            div { style: "flex: 1 1 auto;" }
            if let (true, Some(so_ctx)) = (is_local, settings_open) {
                button {
                    r#type: "button",
                    class: "operon-activity-toggle",
                    "data-testid": "settings-gear",
                    title: "Settings",
                    "aria-label": "Settings",
                    onclick: {
                        let mut so = so_ctx.0;
                        move |_| { so.set(true); }
                    },
                    Icon { name: "settings".to_string() }
                }
            }
            button {
                r#type: "button",
                class: "operon-activity-toggle",
                "data-action": "toggle-sidebar",
                title: "{toggle_label}",
                "aria-label": "{toggle_label}",
                "aria-pressed": if collapsed { "false" } else { "true" },
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
