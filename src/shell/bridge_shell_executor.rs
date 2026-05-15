//! Wire `operon_bash` MCP calls from the claude-code bridge to
//! `operon-plugins-tools::ShellTool`, streaming chunks into the
//! companion UI's `TOOL_STREAM_OUTPUT` signal so the tool card
//! renders live output exactly as it does for the runtime backend.
//!
//! Lifecycle: a `BridgeShellExecutor` is constructed once per session
//! and installed on the session's `PermissionBridge` via
//! `bridge.set_shell_executor(Some(executor))`. Each `tools/call
//! operon_bash` invocation builds a unique `tool_use_id` (or reuses
//! the one claude provided), pushes chunks into `TOOL_STREAM_OUTPUT`
//! as they arrive from `ShellTool::invoke_streaming`, then marks
//! the stream complete and returns the buffered final result.
//!
//! Only relevant when [`crate::shell::auto_approve::AutoApprovePolicy::bash_via_operon`]
//! is `true`; the runtime never reaches this code otherwise.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use futures::future::BoxFuture;
use operon_core::error::OperonResult;
use operon_core::traits::{CancellationToken, ToolChunk, ToolPlugin};
use operon_plugins_claude_code::ShellExecutor;
use operon_plugins_tools::shell::ShellTool;
use serde_json::Value;

use crate::shell::companion_state::{append_tool_chunk, mark_tool_stream_complete};

pub struct BridgeShellExecutor {
    tool: Arc<ShellTool>,
}

impl BridgeShellExecutor {
    pub fn new() -> Self {
        Self {
            tool: Arc::new(ShellTool),
        }
    }
}

impl Default for BridgeShellExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellExecutor for BridgeShellExecutor {
    fn execute<'a>(
        &'a self,
        tool_use_id: String,
        args: Value,
    ) -> BoxFuture<'a, OperonResult<Value>> {
        let tool = self.tool.clone();
        Box::pin(async move {
            // Drop the "arguments" / "input" extras claude tacks on
            // — ShellTool only cares about command/cwd/timeout_ms.
            let shell_args = args.clone();
            // Unbounded chunk channel; a small forwarder task pumps
            // chunks into TOOL_STREAM_OUTPUT so the tool card live
            // region renders incrementally. Dropped after
            // invoke_streaming returns; the receiver task closes
            // naturally.
            let (chunk_tx, mut chunk_rx) =
                tokio::sync::mpsc::unbounded_channel::<ToolChunk>();
            let tool_use_id_for_forward = tool_use_id.clone();
            let forward = tokio::spawn(async move {
                while let Some(chunk) = chunk_rx.recv().await {
                    append_tool_chunk(&tool_use_id_for_forward, &chunk.kind, &chunk.bytes);
                }
            });
            // No per-tool cancel wiring here yet — the claude-code
            // backend doesn't expose `cancel_tool` to the runtime so
            // even if we wired one up, the UI couldn't fire it. A
            // follow-up can plumb through to `AgentBackend::cancel_tool`
            // overridden on `ClaudeCodeChatPlugin` once we have a
            // session→bash_pid registry.
            let ct = CancellationToken::new();
            let result = tool.invoke_streaming(shell_args, ct, chunk_tx).await;
            // Wait for the forwarder to drain — chunk_tx was already
            // dropped at the end of invoke_streaming so the receiver
            // sees None and exits.
            let _ = forward.await;
            mark_tool_stream_complete(&tool_use_id);
            result
        })
    }
}
