//! Capability grants for MCP tools.
//!
//! A `GrantHandler` decides whether to allow a tool invocation. Implementations:
//! - `AutoApproveGrantHandler`: always allows (default for tests; logs a warning).
//! - `DenyAllGrantHandler`: always denies.
//! - `SecretStoreGrantHandler`: looks up `mcp-grant/{server}/{tool}` in a SecretStore;
//!   returns Ok(true) if value is "allow", false otherwise.

use operon_core::error::OperonResult;
use operon_core::secrets::SecretStore;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait GrantHandler: Send + Sync {
    async fn check(&self, server: &str, tool: &str) -> OperonResult<bool>;
}

pub struct AutoApproveGrantHandler;
#[async_trait]
impl GrantHandler for AutoApproveGrantHandler {
    async fn check(&self, server: &str, tool: &str) -> OperonResult<bool> {
        tracing::warn!(target: "operon::mcp", %server, %tool, "auto-approving tool grant; tighten in production");
        Ok(true)
    }
}

pub struct DenyAllGrantHandler;
#[async_trait]
impl GrantHandler for DenyAllGrantHandler {
    async fn check(&self, _server: &str, _tool: &str) -> OperonResult<bool> {
        Ok(false)
    }
}

pub struct SecretStoreGrantHandler {
    secrets: Arc<dyn SecretStore>,
}

impl SecretStoreGrantHandler {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }
    fn key(server: &str, tool: &str) -> String {
        format!("mcp-grant/{server}/{tool}")
    }
    pub async fn record(&self, server: &str, tool: &str, allow: bool) -> OperonResult<()> {
        let v = if allow { "allow" } else { "deny" };
        self.secrets.put(&Self::key(server, tool), v).await
    }
    pub async fn revoke(&self, server: &str, tool: &str) -> OperonResult<()> {
        self.secrets.delete(&Self::key(server, tool)).await
    }
}

#[async_trait]
impl GrantHandler for SecretStoreGrantHandler {
    async fn check(&self, server: &str, tool: &str) -> OperonResult<bool> {
        Ok(matches!(
            self.secrets.get(&Self::key(server, tool)).await?.as_deref(),
            Some("allow")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::secrets::MockSecretStore;

    #[tokio::test]
    async fn auto_approve_returns_true() {
        let g = AutoApproveGrantHandler;
        assert!(g.check("server", "tool").await.unwrap());
    }

    #[tokio::test]
    async fn deny_all_returns_false() {
        let g = DenyAllGrantHandler;
        assert!(!g.check("server", "tool").await.unwrap());
    }

    #[tokio::test]
    async fn secret_store_grant_lifecycle() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
        let g = SecretStoreGrantHandler::new(secrets);
        assert!(!g.check("s", "t").await.unwrap());
        g.record("s", "t", true).await.unwrap();
        assert!(g.check("s", "t").await.unwrap());
        g.revoke("s", "t").await.unwrap();
        assert!(!g.check("s", "t").await.unwrap());
    }

    #[tokio::test]
    async fn secret_store_grant_persists_across_handlers() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
        let g1 = SecretStoreGrantHandler::new(secrets.clone());
        g1.record("s", "t", true).await.unwrap();
        let g2 = SecretStoreGrantHandler::new(secrets);
        assert!(g2.check("s", "t").await.unwrap());
    }
}
