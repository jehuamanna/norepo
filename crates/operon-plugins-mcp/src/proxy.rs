//! McpToolProxy — adapts an MCP-served tool into a `ToolPlugin`.

use operon_core::error::OperonResult;
use operon_core::traits::{
    CancellationToken, Capabilities, McpClient, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use std::sync::Arc;

pub struct McpToolProxy {
    pub client: Arc<dyn McpClient>,
    pub tool: ToolDef,
}

impl McpToolProxy {
    pub fn new(client: Arc<dyn McpClient>, tool: ToolDef) -> Self {
        Self { client, tool }
    }
}

#[async_trait]
impl Plugin for McpToolProxy {
    fn name(&self) -> &str {
        &self.tool.name
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }
}

#[async_trait]
impl ToolPlugin for McpToolProxy {
    fn schema(&self) -> ToolDef {
        self.tool.clone()
    }
    async fn invoke(
        &self,
        args: serde_json::Value,
        ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        self.client.call_tool(&self.tool.name, args, ct).await
    }
}
