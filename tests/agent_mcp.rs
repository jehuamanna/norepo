//! MCP integration tests (Plans-Phase-3).

#![cfg(not(target_arch = "wasm32"))]

use async_trait::async_trait;
use operon_dioxus::agent::mcp::{
    AutoApproveGrantHandler, DenyAllGrantHandler, GrantHandler, McpToolProxy,
    SecretStoreGrantHandler,
};
use operon_dioxus::agent::secrets::{MockSecretStore, SecretStore};
use operon_dioxus::agent::traits::{
    CancellationToken, Capabilities, McpClient, Plugin, ToolDef, ToolPlugin,
};
use operon_dioxus::agent::OperonResult;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// In-process mock McpClient for tests — no subprocess.
struct MockMcpClient {
    tools: Vec<ToolDef>,
    invoked: Arc<AtomicUsize>,
}

#[async_trait]
impl Plugin for MockMcpClient {
    fn name(&self) -> &str {
        "mock-mcp"
    }
    fn version(&self) -> &str {
        "0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::TOOL_USE
    }
}

#[async_trait]
impl McpClient for MockMcpClient {
    async fn connect(&self) -> OperonResult<()> {
        Ok(())
    }
    async fn list_tools(&self) -> OperonResult<Vec<ToolDef>> {
        Ok(self.tools.clone())
    }
    async fn call_tool(
        &self,
        _name: &str,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        self.invoked.fetch_add(1, Ordering::SeqCst);
        Ok(args)
    }
    async fn disconnect(&self) -> OperonResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn proxy_delegates_to_client() {
    let invoked = Arc::new(AtomicUsize::new(0));
    let client: Arc<dyn McpClient> = Arc::new(MockMcpClient {
        tools: vec![ToolDef {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({}),
        }],
        invoked: invoked.clone(),
    });
    let proxy = McpToolProxy::new(
        client.clone(),
        ToolDef {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({}),
        },
    );
    let out = proxy
        .invoke(serde_json::json!({"a": 1}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, serde_json::json!({"a": 1}));
    assert_eq!(invoked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn auto_approve_handler_grants() {
    let g: Arc<dyn GrantHandler> = Arc::new(AutoApproveGrantHandler);
    assert!(g.check("server", "tool").await.unwrap());
}

#[tokio::test]
async fn deny_all_handler_denies() {
    let g: Arc<dyn GrantHandler> = Arc::new(DenyAllGrantHandler);
    assert!(!g.check("server", "tool").await.unwrap());
}

#[tokio::test]
async fn secret_store_grants_persist_across_handlers() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
    let g1 = SecretStoreGrantHandler::new(secrets.clone());
    g1.record("srv", "echo", true).await.unwrap();
    let g2 = SecretStoreGrantHandler::new(secrets);
    assert!(g2.check("srv", "echo").await.unwrap());
}
