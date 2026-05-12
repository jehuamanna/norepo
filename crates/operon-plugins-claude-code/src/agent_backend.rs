//! `AgentBackend` adapter for `ClaudeCodeChatPlugin`.
//!
//! Wraps the existing `send_rich` API in the backend-agnostic
//! `operon_core::AgentEvent` stream so cascade / executor / companion
//! callers can hold an `Arc<dyn AgentBackend>`. Identity translation
//! between `ClaudeCodeEvent` and `AgentEvent` is lossless — the variants
//! match 1:1 with the same payloads.

#![cfg(not(target_arch = "wasm32"))]

use crate::event::ClaudeCodeEvent;
use crate::plugin::ClaudeCodeChatPlugin;
use async_trait::async_trait;
use futures::StreamExt;
use operon_core::agent_event::{AgentBackend, AgentEvent};
use operon_core::error::OperonResult;
use operon_core::traits::CancellationToken;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

impl From<ClaudeCodeEvent> for AgentEvent {
    fn from(ev: ClaudeCodeEvent) -> Self {
        match ev {
            ClaudeCodeEvent::SessionInit { mcp_servers, tools } => {
                AgentEvent::SessionInit { mcp_servers, tools }
            }
            ClaudeCodeEvent::Text(t) => AgentEvent::Text(t),
            ClaudeCodeEvent::Thinking(t) => AgentEvent::Thinking(t),
            ClaudeCodeEvent::ToolUse { id, name, input } => AgentEvent::ToolUse { id, name, input },
            ClaudeCodeEvent::ToolResult { tool_use_id, content, is_error } => {
                AgentEvent::ToolResult { tool_use_id, content, is_error }
            }
            ClaudeCodeEvent::Done { stop_reason, usage } => {
                AgentEvent::Done { stop_reason, usage }
            }
            ClaudeCodeEvent::Error(msg) => AgentEvent::Error(msg),
        }
    }
}

#[async_trait]
impl AgentBackend for ClaudeCodeChatPlugin {
    fn id(&self) -> &str {
        "claude-code"
    }

    async fn bind_session(
        &self,
        operon_session: Uuid,
        cwd: PathBuf,
    ) -> OperonResult<()> {
        // The existing API is sync and infallible — wrap it.
        ClaudeCodeChatPlugin::bind_session(self, operon_session, cwd);
        Ok(())
    }

    async fn send_rich(
        &self,
        prompt: String,
        operon_session: Uuid,
        ct: CancellationToken,
    ) -> OperonResult<UnboundedReceiver<AgentEvent>> {
        // Upstream uses `futures::channel::mpsc::UnboundedReceiver`; our
        // trait surfaces the tokio variant so consumers don't need to know
        // which channel library the backend chose. Forwarder task adapts
        // both directions.
        let mut rx = ClaudeCodeChatPlugin::send_rich(self, prompt, operon_session, ct).await?;
        let (tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        tokio::spawn(async move {
            while let Some(ev) = rx.next().await {
                if tx.send(ev.into()).is_err() {
                    break;
                }
            }
        });
        Ok(out_rx)
    }

    async fn unbind_session(&self, operon_session: Uuid) -> OperonResult<()> {
        ClaudeCodeChatPlugin::unbind_session(self, operon_session);
        Ok(())
    }

    async fn set_session_permission_mode(
        &self,
        operon_session: Uuid,
        mode: Option<String>,
    ) -> OperonResult<()> {
        ClaudeCodeChatPlugin::set_session_permission_mode(self, operon_session, mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::traits::{StopReason, Usage};

    #[test]
    fn from_claude_event_text() {
        let e: AgentEvent = ClaudeCodeEvent::Text("hi".into()).into();
        match e {
            AgentEvent::Text(s) => assert_eq!(s, "hi"),
            _ => panic!(),
        }
    }

    #[test]
    fn from_claude_event_done_carries_usage() {
        let usage = Usage {
            prompt: 1,
            prompt_cached: 0,
            completion: 2,
        };
        let e: AgentEvent = ClaudeCodeEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: Some(usage.clone()),
        }
        .into();
        match e {
            AgentEvent::Done { usage: Some(u), .. } => {
                assert_eq!(u.prompt, 1);
                assert_eq!(u.completion, 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn from_claude_event_tool_use_round_trips_input() {
        let e: AgentEvent = ClaudeCodeEvent::ToolUse {
            id: "1".into(),
            name: "read".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        }
        .into();
        match e {
            AgentEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "1");
                assert_eq!(name, "read");
                assert_eq!(input["path"].as_str(), Some("/tmp/x"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn from_claude_event_error() {
        let e: AgentEvent = ClaudeCodeEvent::Error("boom".into()).into();
        assert!(matches!(e, AgentEvent::Error(s) if s == "boom"));
    }
}
