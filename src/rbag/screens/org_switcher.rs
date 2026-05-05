use dioxus::prelude::*;

use crate::rbag::api::ApiClient;
use crate::rbag::state::AppState;

#[component]
pub fn OrgSwitcher() -> Element {
    let mut state = use_context::<Signal<AppState>>();
    let client = use_context::<Signal<ApiClient>>();
    let s = state();
    let memberships = s
        .identity
        .as_ref()
        .map(|i| i.memberships.clone())
        .unwrap_or_default();
    let active = s
        .identity
        .as_ref()
        .and_then(|i| i.active_org_id.clone())
        .unwrap_or_default();

    let on_change = move |evt: FormEvent| {
        let chosen = evt.value();
        let chosen_for_api = chosen.clone();
        let api = client();
        spawn(async move {
            let _ = api.set_active_org(&chosen_for_api).await;
        });
        state.with_mut(|s| {
            if let Some(i) = s.identity.as_mut() {
                i.active_org_id = Some(chosen.clone());
                if let Some(m) = i.memberships.iter().find(|m| m.org_id == chosen) {
                    i.role_in_active_org = Some(m.role.clone());
                }
            }
        });
    };

    rsx! {
        div { class: "rbag-org-switcher",
            select { onchange: on_change,
                {memberships.iter().map(|m| {
                    let selected = m.org_id == active;
                    rsx! {
                        option { value: "{m.org_id}", selected: selected, "{m.org_id} ({m.role})" }
                    }
                })}
            }
        }
    }
}
