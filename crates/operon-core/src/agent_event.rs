//! Backend-agnostic event stream for cascade / executor consumers.
//!
//! Both the legacy `ClaudeCodeChatPlugin` (subprocess to the `claude` CLI)
//! and the new in-process `AgentRuntime` produce the same shape of events.
//! Consumers (cascade.rs, executor.rs, companion_chat.rs) used to take
//! `Arc<ClaudeCodeChatPlugin>` directly; the `AgentBackend` trait lets them
//! take `Arc<dyn AgentBackend>` instead, so the cutover (Slice A14) is a
//! one-line change at each call site.
//!
//! `AgentEvent` mirrors `ClaudeCodeEvent` exactly. Each variant has a `From`
//! conversion so existing `match` arms continue to compile after a single
//! `.map(AgentEvent::from)` adapter on the receiver.

#![cfg(not(target_arch = "wasm32"))]

use crate::error::OperonResult;
use crate::traits::{CancellationToken, StopReason, Usage};
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

/// Connection state of an MCP server reported in claude's `system/init`
/// event. Drives the active/inactive indicator in the MCP settings panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerStatus {
    pub name: String,
    /// Raw status string from claude — typically `connected`, `failed`,
    /// `needs-auth`, or similar. Surfaced verbatim so the UI can render
    /// new states claude introduces without a code change.
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Emitted once per turn when claude reports `system/init`. Carries the
    /// MCP servers that connected for this session and the full tool list
    /// (including `mcp__<server>__<tool>` entries). The companion uses this
    /// to drive the MCP panel's active/tools indicators.
    SessionInit {
        mcp_servers: Vec<McpServerStatus>,
        tools: Vec<String>,
    },
    /// Streamed assistant text delta.
    Text(String),
    /// Extended-thinking content block. Render as collapsible reasoning.
    Thinking(String),
    /// Tool invocation — render as a card; correlate with the matching
    /// `ToolResult` by `id` ↔ `tool_use_id`.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Result of a previous `ToolUse`.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Streaming bytes from a running tool — emitted between
    /// `ToolUse` and `ToolResult` for tools that opt into chunk
    /// streaming. Companion UI accumulates these per `tool_use_id`
    /// into a live output region so long Bash commands surface
    /// progress instead of looking stuck.
    ToolChunk {
        tool_use_id: String,
        /// `"stdout"` or `"stderr"` for shell tools; tool-specific
        /// labels otherwise.
        kind: String,
        bytes: Vec<u8>,
    },
    /// Turn finished. `usage` lands in the cost meter.
    Done {
        stop_reason: StopReason,
        usage: Option<Usage>,
    },
    /// Permission ask — the runtime needs the user to authorise a tool call
    /// before it proceeds. Slice A12 wires the inline UI prompt.
    PermissionRequest {
        id: String,
        title: String,
        kind: String,
        locations: Vec<String>,
        raw_input: serde_json::Value,
    },
    /// Protocol-level error. The companion renders as an inline system
    /// message; turn ends.
    Error(String),
}

/// Trait every agent backend implements so cascade / executor / companion
/// can dispatch over `Arc<dyn AgentBackend>`.
///
/// Implementations:
/// - `ClaudeCodeChatPlugin` (existing subprocess; in `operon-plugins-claude-code`).
/// - `RuntimeAgentBackend` (new in-process runtime; in `operon-plugins-tools`).
///
/// Both speak the same `AgentEvent` stream so consumers don't need to know
/// which backend they're talking to.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Backend id — `"claude-code"` or `"runtime"`.
    fn id(&self) -> &str;

    /// Bind a session to a working directory. Both backends store this
    /// per-session so subsequent `send_rich` calls inherit the cwd.
    async fn bind_session(&self, _operon_session: Uuid, _cwd: std::path::PathBuf) -> OperonResult<()> {
        Ok(())
    }

    /// Send a single user prompt and return a stream of `AgentEvent`s.
    /// Cancelling `ct` aborts mid-turn (kills the subprocess for
    /// claude-code, drops the runtime task for the in-process backend).
    async fn send_rich(
        &self,
        prompt: String,
        operon_session: Uuid,
        ct: CancellationToken,
    ) -> OperonResult<UnboundedReceiver<AgentEvent>>;

    /// Tear down per-session state. Default no-op for backends that don't
    /// need it; claude-code uses this to clean up subprocess bookkeeping.
    async fn unbind_session(&self, _operon_session: Uuid) -> OperonResult<()> {
        Ok(())
    }

    /// Cancel a single in-flight tool call by `tool_use_id` without
    /// killing the rest of the turn. Returns `true` when a matching
    /// in-flight tool was found and signalled. Default returns
    /// `false` — claude-code backend can't reach into its own
    /// subprocess at this granularity, so cancelling a single tool
    /// there always degrades to "stop the whole turn". The in-process
    /// runtime backend overrides this with a real per-tool cancel.
    async fn cancel_tool(&self, _operon_session: Uuid, _tool_use_id: &str) -> bool {
        false
    }

    /// Override the per-session permission mode (e.g. force `acceptEdits`
    /// during cascade runs). `None` clears any prior override. Default
    /// no-op for backends without a mode concept.
    async fn set_session_permission_mode(
        &self,
        _operon_session: Uuid,
        _mode: Option<String>,
    ) -> OperonResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_variants_compile() {
        let _ = AgentEvent::SessionInit {
            mcp_servers: vec![McpServerStatus {
                name: "fs".into(),
                status: "connected".into(),
            }],
            tools: vec!["mcp__fs__read".into()],
        };
        let _ = AgentEvent::Text("hi".into());
        let _ = AgentEvent::Thinking("…".into());
        let _ = AgentEvent::ToolUse {
            id: "1".into(),
            name: "read".into(),
            input: serde_json::json!({}),
        };
        let _ = AgentEvent::ToolResult {
            tool_use_id: "1".into(),
            content: "ok".into(),
            is_error: false,
        };
        let _ = AgentEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: Some(Usage::default()),
        };
        let _ = AgentEvent::PermissionRequest {
            id: "p".into(),
            title: "shell ls".into(),
            kind: "shell".into(),
            locations: vec![],
            raw_input: serde_json::json!({}),
        };
        let _ = AgentEvent::Error("oops".into());
    }
}
