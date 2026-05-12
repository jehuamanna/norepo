//! Providers settings section — vertical list of `ProviderCard`s.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use super::provider_card::ProviderCard;
use super::service::ProviderId;

#[component]
pub fn ProvidersSection() -> Element {
    rsx! {
        div { class: "operon-settings-providers",
            "data-testid": "providers-section",
            h3 { class: "operon-modal-section",
                style: "margin-top: 1rem; font-weight: 600;",
                "Provider API keys"
            }
            p { class: "operon-modal-help",
                style: "font-size: 0.8em; color: var(--operon-fg-muted, #666); margin-bottom: 0.5rem;",
                "Stored in the OS keyring. Falls back to env vars (e.g. ANTHROPIC_API_KEY) when the keyring is unavailable."
            }
            for provider in ProviderId::all().iter().copied() {
                ProviderCard { provider }
            }
        }
    }
}
