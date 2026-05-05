use crate::agent::error::OperonResult;
use async_trait::async_trait;
use bitflags::bitflags;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use uuid::Uuid;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Capabilities: u32 {
        const STREAMING       = 1 << 0;
        const TOOL_USE        = 1 << 1;
        const VISION          = 1 << 2;
        const PROMPT_CACHE    = 1 << 3;
        const VECTOR_SEARCH   = 1 << 4;
        const MULTI_TENANT    = 1 << 5;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub created_at_ms: u64,
    pub session: Uuid,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    User,
    Project(Uuid),
    Team(Uuid),
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub message: Message,
    pub score: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt: u64,
    pub prompt_cached: u64,
    pub completion: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    Tool,
    Other(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChatDelta {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Stop {
        reason: StopReason,
        usage: Option<Usage>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
}

pub type ChatStream = Pin<Box<dyn Stream<Item = OperonResult<ChatDelta>> + Send>>;

pub use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    async fn init(&self) -> OperonResult<()> {
        Ok(())
    }
    async fn shutdown(&self) -> OperonResult<()> {
        Ok(())
    }
    async fn health(&self) -> OperonResult<()> {
        Ok(())
    }
}

#[async_trait]
pub trait ChatPlugin: Plugin {
    async fn complete(&self, req: ChatRequest, ct: CancellationToken) -> OperonResult<ChatStream>;
}

#[async_trait]
pub trait ToolPlugin: Plugin {
    fn schema(&self) -> ToolDef;
    async fn invoke(
        &self,
        args: serde_json::Value,
        ct: CancellationToken,
    ) -> OperonResult<serde_json::Value>;
}

#[async_trait]
pub trait MemoryPlugin: Plugin {
    async fn write(&self, scope: Scope, msg: Message) -> OperonResult<Uuid>;
    async fn read(&self, scope: Scope, id: Uuid) -> OperonResult<Option<Message>>;
    async fn search(&self, scope: Scope, query: &str, k: usize) -> OperonResult<Vec<Hit>>;
    async fn delete(&self, scope: Scope, id: Uuid) -> OperonResult<()>;
}

#[async_trait]
pub trait McpClient: Plugin {
    async fn connect(&self) -> OperonResult<()>;
    async fn list_tools(&self) -> OperonResult<Vec<ToolDef>>;
    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        ct: CancellationToken,
    ) -> OperonResult<serde_json::Value>;
    async fn disconnect(&self) -> OperonResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_compose() {
        let caps = Capabilities::STREAMING | Capabilities::TOOL_USE | Capabilities::PROMPT_CACHE;
        assert!(caps.contains(Capabilities::STREAMING));
        assert!(caps.contains(Capabilities::TOOL_USE));
        assert!(caps.contains(Capabilities::PROMPT_CACHE));
        assert!(!caps.contains(Capabilities::VISION));
    }

    #[test]
    fn message_serde_roundtrip() {
        let m = Message {
            id: Uuid::new_v4(),
            role: Role::User,
            content: vec![ContentBlock::Text("hi".into())],
            created_at_ms: 1,
            session: Uuid::new_v4(),
            metadata: Default::default(),
        };
        let s = serde_json::to_string(&m).unwrap();
        let m2: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(m.id, m2.id);
        assert!(matches!(m2.role, Role::User));
    }

    #[test]
    fn content_block_variants_serde() {
        for cb in [
            ContentBlock::Text("x".into()),
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "t".into(),
                input: serde_json::json!({"x": 1}),
            },
            ContentBlock::ToolResult {
                tool_use_id: "a".into(),
                content: "ok".into(),
                is_error: false,
            },
        ] {
            let s = serde_json::to_string(&cb).unwrap();
            let _back: ContentBlock = serde_json::from_str(&s).unwrap();
        }
    }
}
