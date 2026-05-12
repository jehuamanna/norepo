//! Rich per-turn events emitted by `ClaudeCodeChatPlugin::send_rich`.
//!
//! `ChatPlugin::complete` is a thin text-only adapter on top of this stream
//! (kept around for backward compat with the live integration test and for
//! any agent-runtime caller that wants the trait-level interface). The
//! companion pane subscribes to `ClaudeCodeEvent` directly so it can
//! render tool-use cards, thinking blocks, and the cost meter without the
//! lossy ChatDelta translation.

#![cfg(not(target_arch = "wasm32"))]

use operon_core::agent_event::McpServerStatus;
use operon_core::traits::{StopReason, Usage};

#[derive(Debug, Clone)]
pub enum ClaudeCodeEvent {
    /// Initial `system/init` line from claude's stream-json. Reports the
    /// MCP servers that connected and the full tool inventory available to
    /// this turn. Surfaced so the companion's MCP panel can render live
    /// "is this server up + which tools" indicators.
    SessionInit {
        mcp_servers: Vec<McpServerStatus>,
        tools: Vec<String>,
    },
    /// Streamed assistant text delta. Multiple `Text` events from one turn
    /// concat to the full assistant response (which may also be
    /// interleaved with `ToolUse` / `Thinking`).
    Text(String),
    /// Extended-thinking content block, surfaced when claude is run with
    /// `--include-partial-messages` (or as a final block at end-of-turn
    /// otherwise). The companion renders these dim + collapsible.
    Thinking(String),
    /// Claude is about to invoke (or has invoked) a tool. The companion
    /// renders this as a card; the matching `ToolResult` (correlated by
    /// `id` ↔ `tool_use_id`) populates the card's expanded body.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// The result of a previous `ToolUse`. Stream-json reports these as
    /// claude-side `user` events with `tool_result` content blocks.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Turn finished. `usage` accumulates into the cost meter; the
    /// `claude_session_id` carried in the underlying result event is
    /// already cached into the plugin's per-Operon-session binding.
    Done {
        stop_reason: StopReason,
        usage: Option<Usage>,
    },
    /// A protocol-level error from claude (parse failure, server error,
    /// etc.). Companion renders as an inline system message; turn ends.
    Error(String),
}
