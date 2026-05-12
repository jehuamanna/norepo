//! Settings UI (Slice A4b) — provider API key management.
//!
//! Mounted inside the existing `SettingsPanel` modal in `desktop.rs` as a
//! "Provider API keys" section. The `SettingsService` is provided via
//! Dioxus context from `desktop.rs::Workspace` so test harnesses can hand
//! in a mock secret store.

#![cfg(not(target_arch = "wasm32"))]

pub mod provider_card;
pub mod providers;
pub mod service;

pub use providers::ProvidersSection;
pub use service::{ProviderId, SettingsService, VerifyOutcome};

/// Dioxus context wrapper so the settings modal can pull a `SettingsService`
/// without re-building one per render.
#[derive(Clone)]
pub struct SettingsServiceCtx(pub SettingsService);
