//! Activity bar: pinned vertical icon column on the left edge.
//!
//! Iterates `UIPlugin` contributions for [`PluginSurface::ActivityBar`] and renders each
//! as a clickable icon. Clicking an icon either activates its plugin's side-bar panel or,
//! if it's already active, collapses the side bar.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::{PluginRegistry, PluginSurface};
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};

#[component]
pub fn ActivityBar() -> Element {
    let registry: Rc<PluginRegistry> = use_context();
    let ActiveActivity(mut active) = use_context();
    let LastActiveActivity(mut last) = use_context();

    let active_id = active.read().clone();

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
        }
    }
}
