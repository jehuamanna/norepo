use dioxus::prelude::*;

use crate::rbag::state::AppState;

#[component]
pub fn MeBadge() -> Element {
    let state = use_context::<Signal<AppState>>();
    let s = state();
    rsx! {
        div { class: "rbag-me-badge",
            if let Some(id) = s.identity.as_ref() {
                if let Some(role) = id.role_in_active_org.as_deref() {
                    span { class: "role", "{role}" }
                }
                span { class: "user-id", "{id.user_id}" }
            } else {
                span { class: "anon", "Signed out" }
            }
        }
    }
}
