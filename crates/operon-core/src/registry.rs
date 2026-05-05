use crate::traits::{ChatPlugin, McpClient, MemoryPlugin, ToolPlugin};
use std::sync::Arc;

pub struct AgentRegistry {
    pub chat: Vec<Arc<dyn ChatPlugin>>,
    pub tools: Vec<Arc<dyn ToolPlugin>>,
    pub memory: Vec<Arc<dyn MemoryPlugin>>,
    pub mcp: Vec<Arc<dyn McpClient>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            chat: Vec::new(),
            tools: Vec::new(),
            memory: Vec::new(),
            mcp: Vec::new(),
        }
    }

    pub fn register_chat(&mut self, p: Arc<dyn ChatPlugin>) {
        self.chat.push(p);
    }
    pub fn register_tool(&mut self, p: Arc<dyn ToolPlugin>) {
        self.tools.push(p);
    }
    pub fn register_memory(&mut self, p: Arc<dyn MemoryPlugin>) {
        self.memory.push(p);
    }
    pub fn register_mcp(&mut self, p: Arc<dyn McpClient>) {
        self.mcp.push(p);
    }

    pub fn chat_by_name(&self, name: &str) -> Option<Arc<dyn ChatPlugin>> {
        self.chat.iter().find(|p| p.name() == name).cloned()
    }
    pub fn tool_by_name(&self, name: &str) -> Option<Arc<dyn ToolPlugin>> {
        self.tools.iter().find(|p| p.name() == name).cloned()
    }
    pub fn memory_by_name(&self, name: &str) -> Option<Arc<dyn MemoryPlugin>> {
        self.memory.iter().find(|p| p.name() == name).cloned()
    }
    pub fn mcp_by_name(&self, name: &str) -> Option<Arc<dyn McpClient>> {
        self.mcp.iter().find(|p| p.name() == name).cloned()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn register_agent_plugins() -> AgentRegistry {
    AgentRegistry::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OperonResult;
    use crate::traits::{
        Capabilities, ChatRequest, ChatStream, Plugin,
    };
    use async_trait::async_trait;

    struct DummyChat {
        n: String,
    }

    #[async_trait]
    impl Plugin for DummyChat {
        fn name(&self) -> &str {
            &self.n
        }
        fn version(&self) -> &str {
            "0"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::STREAMING
        }
    }

    #[async_trait]
    impl ChatPlugin for DummyChat {
        async fn complete(
            &self,
            _req: ChatRequest,
            _ct: crate::traits::CancellationToken,
        ) -> OperonResult<ChatStream> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut r = AgentRegistry::new();
        r.register_chat(Arc::new(DummyChat { n: "echo".into() }));
        assert!(r.chat_by_name("echo").is_some());
        assert!(r.chat_by_name("missing").is_none());
    }

    #[test]
    fn empty_registry_lookups_return_none() {
        let r = AgentRegistry::new();
        assert!(r.chat_by_name("any").is_none());
        assert!(r.tool_by_name("any").is_none());
        assert!(r.memory_by_name("any").is_none());
        assert!(r.mcp_by_name("any").is_none());
    }

    #[test]
    fn register_agent_plugins_returns_empty() {
        let r = register_agent_plugins();
        assert!(r.chat.is_empty());
        assert!(r.tools.is_empty());
    }
}
