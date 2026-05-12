//! Settings service — owns the `LayeredSecretStore` and verification HTTP
//! client used by the Provider settings card. Provided via Dioxus context.
//!
//! Slice A4b: a thin wrapper so UI components don't pull in `keyring` or
//! `reqwest` directly. Tests hand in a `MockSecretStore`; production hands
//! in the `LayeredSecretStore` built in `desktop.rs`.

#![cfg(not(target_arch = "wasm32"))]

use operon_core::secrets::SecretStore;
use std::sync::Arc;

/// Identifier of a provider whose key the settings UI manages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Anthropic,
    OpenAI,
    Google,
    Tavily,
}

impl ProviderId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAI => "OpenAI",
            Self::Google => "Google Gemini",
            Self::Tavily => "Tavily (web search)",
        }
    }

    /// SecretStore key under which the API key is stored.
    pub fn secret_key(self) -> &'static str {
        match self {
            Self::Anthropic => operon_core::secrets::keys::ANTHROPIC_API_KEY,
            Self::OpenAI => operon_core::secrets::keys::OPENAI_API_KEY,
            Self::Google => operon_core::secrets::keys::GOOGLE_API_KEY,
            Self::Tavily => operon_core::secrets::keys::TAVILY_API_KEY,
        }
    }

    pub fn all() -> [ProviderId; 4] {
        [Self::Anthropic, Self::OpenAI, Self::Google, Self::Tavily]
    }
}

/// Result of a `verify` call against a provider's models endpoint.
#[derive(Clone, Debug)]
pub enum VerifyOutcome {
    Ok,
    Failed(String),
}

#[derive(Clone)]
pub struct SettingsService {
    pub secrets: Arc<dyn SecretStore>,
    pub http: reqwest::Client,
}

impl SettingsService {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { secrets, http }
    }

    pub async fn set_key(&self, provider: ProviderId, value: &str) -> Result<(), String> {
        self.secrets
            .put(provider.secret_key(), value)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn remove_key(&self, provider: ProviderId) -> Result<(), String> {
        self.secrets
            .delete(provider.secret_key())
            .await
            .map_err(|e| e.to_string())
    }

    /// Returns a 4-character-tail mask (`sk-…XXXX`) when a key is configured,
    /// `None` otherwise. Never returns the full secret.
    pub async fn masked_key(&self, provider: ProviderId) -> Option<String> {
        let v = self
            .secrets
            .get(provider.secret_key())
            .await
            .ok()
            .flatten()?;
        Some(mask(&v))
    }

    /// Best-effort liveness check against the provider's public endpoint.
    pub async fn verify(&self, provider: ProviderId) -> VerifyOutcome {
        let key = match self.secrets.get(provider.secret_key()).await {
            Ok(Some(k)) => k,
            Ok(None) => return VerifyOutcome::Failed("key not configured".into()),
            Err(e) => return VerifyOutcome::Failed(format!("secret read: {e}")),
        };
        let res = match provider {
            ProviderId::Anthropic => self
                .http
                .get("https://api.anthropic.com/v1/models")
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await,
            ProviderId::OpenAI => self
                .http
                .get("https://api.openai.com/v1/models")
                .bearer_auth(&key)
                .send()
                .await,
            ProviderId::Google => self
                .http
                .get(format!(
                    "https://generativelanguage.googleapis.com/v1beta/models?key={}",
                    key
                ))
                .send()
                .await,
            ProviderId::Tavily => self
                .http
                .post("https://api.tavily.com/search")
                .json(&serde_json::json!({
                    "api_key": key,
                    "query": "ping",
                    "max_results": 1,
                }))
                .send()
                .await,
        };
        match res {
            Ok(r) if r.status().is_success() => VerifyOutcome::Ok,
            Ok(r) => VerifyOutcome::Failed(format!("HTTP {}", r.status().as_u16())),
            Err(e) => VerifyOutcome::Failed(format_error_chain(&e)),
        }
    }
}

/// Walk the error source chain so the user sees the underlying cause
/// (rustls handshake failure, DNS resolution error, proxy refusal) instead
/// of reqwest's generic "error sending request for url …".
fn format_error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

fn mask(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 4 {
        return "•••".to_string();
    }
    let tail = &trimmed[trimmed.len() - 4..];
    format!("•••••••{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::secrets::MockSecretStore;

    #[tokio::test]
    async fn set_then_masked_returns_tail() {
        let store: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
        let svc = SettingsService::new(store);
        svc.set_key(ProviderId::Anthropic, "sk-ant-abcdef1234")
            .await
            .unwrap();
        let masked = svc.masked_key(ProviderId::Anthropic).await.unwrap();
        assert!(masked.ends_with("1234"), "got {masked}");
        assert!(!masked.contains("abcdef"));
    }

    #[tokio::test]
    async fn masked_returns_none_when_unset() {
        let store: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
        let svc = SettingsService::new(store);
        assert!(svc.masked_key(ProviderId::OpenAI).await.is_none());
    }

    #[tokio::test]
    async fn remove_then_masked_returns_none() {
        let store: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
        let svc = SettingsService::new(store);
        svc.set_key(ProviderId::OpenAI, "sk-foo-bar").await.unwrap();
        svc.remove_key(ProviderId::OpenAI).await.unwrap();
        assert!(svc.masked_key(ProviderId::OpenAI).await.is_none());
    }

    #[test]
    fn provider_id_secret_key_is_namespaced() {
        assert!(ProviderId::Anthropic.secret_key().starts_with("provider/"));
    }
}
