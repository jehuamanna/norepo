use dioxus::prelude::*;

use super::api::ApiClient;
use super::types::MePayload;

/// Central AppState for the RBAG/ODU/TPN frontend. Held in a `Signal` and
/// shared via `use_context_provider` at the app root.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppState {
    pub identity: Option<MePayload>,
    pub session_token: Option<String>,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    NonLocal,
    Local,
}

impl AppState {
    pub fn is_authenticated(&self) -> bool {
        match self.mode {
            Mode::Local => true,
            Mode::NonLocal => self.identity.is_some(),
        }
    }
}

/// Provider component that injects the AppState signal + a default ApiClient
/// into every descendant. Mount near the router root.
#[component]
pub fn AppStateProvider(children: Element, base_url: String) -> Element {
    let state: Signal<AppState> = use_signal(AppState::default);
    use_context_provider(|| state);
    let client: Signal<ApiClient> = use_signal(move || ApiClient::new(base_url.clone()));
    use_context_provider(|| client);
    rsx! { {children} }
}
