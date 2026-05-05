//! Echo plugins for testing the agent runtime without any LLM/network call.
//!
//! `EchoChatPlugin::new(script)` returns a chat plugin that emits a scripted
//! sequence of `ChatDelta`s on each call.
//!
//! `EchoToolPlugin::new("name")` returns a tool that echoes its input as output.

use crate::agent::error::{OperonError, OperonResult};
use crate::agent::traits::{
    CancellationToken, Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, Plugin,
    StopReason, ToolDef, ToolPlugin, Usage,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

pub struct EchoChatPlugin {
    name: String,
    script: Mutex<VecDeque<Vec<ChatDelta>>>,
}

impl EchoChatPlugin {
    pub fn new(name: impl Into<String>, script: Vec<Vec<ChatDelta>>) -> Self {
        Self {
            name: name.into(),
            script: Mutex::new(VecDeque::from(script)),
        }
    }

    /// Convenience: a turn that emits `text`, then a tool_use, then stop.
    pub fn turn_with_tool(
        text: &str,
        tool_id: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Vec<ChatDelta> {
        vec![
            ChatDelta::Text(text.to_string()),
            ChatDelta::ToolUse {
                id: tool_id.to_string(),
                name: tool_name.to_string(),
                input,
            },
            ChatDelta::Stop {
                reason: StopReason::Tool,
                usage: Some(Usage {
                    prompt: 10,
                    prompt_cached: 0,
                    completion: 5,
                }),
            },
        ]
    }

    /// Convenience: a turn that emits `text` and ends the conversation.
    pub fn turn_done(text: &str) -> Vec<ChatDelta> {
        vec![
            ChatDelta::Text(text.to_string()),
            ChatDelta::Stop {
                reason: StopReason::EndTurn,
                usage: Some(Usage {
                    prompt: 10,
                    prompt_cached: 5,
                    completion: 5,
                }),
            },
        ]
    }
}

#[async_trait]
impl Plugin for EchoChatPlugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAMING | Capabilities::TOOL_USE
    }
}

#[async_trait]
impl ChatPlugin for EchoChatPlugin {
    async fn complete(
        &self,
        _req: ChatRequest,
        _ct: CancellationToken,
    ) -> OperonResult<ChatStream> {
        let next = self
            .script
            .lock()
            .map_err(|_| OperonError::Provider {
                provider: "echo".into(),
                message: "lock poisoned".into(),
                retryable: false,
            })?
            .pop_front()
            .ok_or_else(|| OperonError::Provider {
                provider: "echo".into(),
                message: "script exhausted".into(),
                retryable: false,
            })?;
        let stream = futures::stream::iter(next.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

pub struct EchoToolPlugin {
    name: String,
}

impl EchoToolPlugin {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Plugin for EchoToolPlugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }
}

#[async_trait]
impl ToolPlugin for EchoToolPlugin {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: self.name.clone(),
            description: "echoes input as output".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }
    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_chat_emits_scripted_turn() {
        let plugin = EchoChatPlugin::new("echo", vec![EchoChatPlugin::turn_done("hello")]);
        let req = ChatRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let mut stream = plugin
            .complete(req, CancellationToken::new())
            .await
            .unwrap();
        use futures::StreamExt;
        let mut deltas = Vec::new();
        while let Some(d) = stream.next().await {
            deltas.push(d.unwrap());
        }
        assert_eq!(deltas.len(), 2);
        assert!(matches!(deltas[0], ChatDelta::Text(_)));
        assert!(matches!(deltas[1], ChatDelta::Stop { .. }));
    }

    #[tokio::test]
    async fn echo_chat_script_exhausted_errors() {
        let plugin = EchoChatPlugin::new("echo", vec![]);
        let req = ChatRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let r = plugin.complete(req, CancellationToken::new()).await;
        assert!(matches!(r, Err(OperonError::Provider { .. })));
    }

    #[tokio::test]
    async fn echo_tool_returns_input() {
        let t = EchoToolPlugin::new("e");
        let out = t
            .invoke(serde_json::json!({"x": 1}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!({"x": 1}));
    }
}
