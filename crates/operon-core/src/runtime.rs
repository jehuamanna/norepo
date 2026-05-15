//! AgentRuntime — ReAct loop on top of ChatPlugin + ToolPlugin + MemoryPlugin.

use crate::budget::Budget;
use crate::bus::{BusEvent, EventBus};
use crate::error::OperonResult;
use crate::permission::{AskInput, PermissionDecision, PermissionGate};
use crate::traits::{
    CancellationToken, ChatDelta, ChatPlugin, ChatRequest, ContentBlock, MemoryPlugin, Message,
    Role, Scope, ToolPlugin, Usage,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Step {
    Started,
    StreamDelta(String),
    /// Extended-thinking content streamed by the model (Claude `thinking` blocks,
    /// OpenAI o-series reasoning summaries, Gemini equivalents). Distinct from
    /// `StreamDelta` so the UI can render it as collapsible reasoning rather
    /// than mixing it into the visible response.
    ///
    /// Added in Slice A0 — emitted starting in Slice A1 once provider plugins
    /// surface thinking deltas.
    Thinking(String),
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        output: serde_json::Value,
        is_error: bool,
    },
    /// Streaming bytes from a running tool (e.g. stdout/stderr of a Bash
    /// command). Emitted between `ToolCall` and `ToolResult` for tools
    /// that opt into streaming via `ToolPlugin::invoke_streaming`. The
    /// UI accumulates these per `tool_use_id` to render a live
    /// terminal-style output region; the final `ToolResult.output`
    /// remains the source of truth for the model's view of what
    /// happened.
    ToolChunk {
        tool_use_id: String,
        /// `"stdout"` or `"stderr"` for shell-style tools; arbitrary
        /// labels for other tools that surface multi-stream output.
        kind: String,
        bytes: Vec<u8>,
    },
    /// Agent is asking the user to approve a privileged tool call. The runtime
    /// blocks (via `crate::permission::PermissionGate`) until a decision arrives
    /// or the request times out.
    ///
    /// Added in Slice A0 — emitted starting in Slice A3 when the permission
    /// gate is wired into the run loop.
    PermissionRequest {
        id: String,
        title: String,
        kind: String,
        locations: Vec<String>,
        raw_input: serde_json::Value,
    },
    Done(StopReason),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    BudgetExceeded(String),
    Cancelled,
    Error(String),
}

pub struct AgentRuntime {
    pub chat: Arc<dyn ChatPlugin>,
    pub tools: Vec<Arc<dyn ToolPlugin>>,
    pub memory: Arc<dyn MemoryPlugin>,
    pub bus: EventBus,
    pub max_iterations: u32,
    /// Per-runtime permission gate. Defaults to an auto-allow gate so existing
    /// callers see no behaviour change. Slice A12 wires the UI to a real gate.
    pub permission: PermissionGate,
    /// Per-tool-call cancellation handles, keyed by `tool_use_id`.
    /// Populated by the agent loop just before each `invoke_streaming`
    /// and cleared on completion. The UI's "Cancel this tool" button
    /// looks up the entry and fires it without killing the whole turn.
    /// `Arc<Mutex<…>>` because two callers (the loop and the cancel
    /// button) need to write/read concurrently across threads.
    pub tool_cancellations: Arc<std::sync::Mutex<HashMap<String, CancellationToken>>>,
}

impl AgentRuntime {
    pub fn new(
        chat: Arc<dyn ChatPlugin>,
        tools: Vec<Arc<dyn ToolPlugin>>,
        memory: Arc<dyn MemoryPlugin>,
        bus: EventBus,
    ) -> Self {
        Self {
            chat,
            tools,
            memory,
            bus,
            max_iterations: 32,
            permission: PermissionGate::default(),
            tool_cancellations: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn with_permission(mut self, gate: PermissionGate) -> Self {
        self.permission = gate;
        self
    }

    /// Cancel a single in-flight tool call. Returns `true` when a
    /// matching handle existed and was fired; `false` when the tool
    /// already completed or never started. Wired to the UI's
    /// tool-card Cancel button via the runtime backend.
    pub fn cancel_tool(&self, tool_use_id: &str) -> bool {
        let Ok(map) = self.tool_cancellations.lock() else {
            return false;
        };
        match map.get(tool_use_id) {
            Some(ct) => {
                ct.cancel();
                true
            }
            None => false,
        }
    }

    fn now_ms() -> u64 {
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn tool_by_name(&self, name: &str) -> Option<Arc<dyn ToolPlugin>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Run the ReAct loop. Returns a Stream<Item = Step>.
    ///
    /// The stream is driven by an internal task spawned via `tokio::spawn`. On WASM
    /// the runtime would need `wasm_bindgen_futures::spawn_local`; that arrives with
    /// the Companion Area in Plans-Phase-4.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run(
        self: Arc<Self>,
        session: Uuid,
        scope: Scope,
        prompt: String,
        budget: Budget,
        ct: CancellationToken,
    ) -> tokio_stream::wrappers::ReceiverStream<Step> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Step>(64);
        let runtime = self.clone();
        let bus = runtime.bus.clone();
        tokio::spawn(async move {
            let _ = run_loop(runtime, session, scope, prompt, budget, ct, bus, tx).await;
        });
        tokio_stream::wrappers::ReceiverStream::new(rx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn run_loop(
    runtime: Arc<AgentRuntime>,
    session: Uuid,
    scope: Scope,
    prompt: String,
    mut budget: Budget,
    ct: CancellationToken,
    bus: EventBus,
    tx: tokio::sync::mpsc::Sender<Step>,
) -> OperonResult<()> {
    tracing::info!(target: "operon::agent", %session, "agent.run.start");
    let _ = tx.send(Step::Started).await;
    bus.publish(BusEvent::AgentStarted { session });

    // Seed conversation with the user prompt.
    let mut messages: Vec<Message> = vec![Message {
        id: Uuid::new_v4(),
        role: Role::User,
        content: vec![ContentBlock::Text(prompt.clone())],
        created_at_ms: AgentRuntime::now_ms(),
        session,
        metadata: Default::default(),
    }];
    let user_msg_id = messages[0].id;
    if let Ok(_id) = runtime.memory.write(scope.clone(), messages[0].clone()).await {
        bus.publish(BusEvent::MemoryWritten {
            session,
            scope: format!("{:?}", scope),
            id: user_msg_id,
        });
    }

    let mut step_n: u32 = 0;
    loop {
        if step_n >= runtime.max_iterations {
            let reason = format!("max_iterations ({})", runtime.max_iterations);
            let _ = tx.send(Step::Done(StopReason::BudgetExceeded(reason.clone()))).await;
            bus.publish(BusEvent::BudgetExceeded { session, reason });
            return Ok(());
        }
        if ct.is_cancelled() {
            let _ = tx.send(Step::Done(StopReason::Cancelled)).await;
            bus.publish(BusEvent::Cancelled { session });
            return Ok(());
        }
        if let Some(reason) = budget.is_exceeded() {
            let _ = tx
                .send(Step::Done(StopReason::BudgetExceeded(reason.to_string())))
                .await;
            bus.publish(BusEvent::BudgetExceeded {
                session,
                reason: reason.to_string(),
            });
            return Ok(());
        }

        tracing::info!(target: "operon::agent", %session, step = step_n, "agent.step");

        // ---- Chat call ----
        let req = ChatRequest {
            system: None,
            messages: messages.clone(),
            tools: runtime.tools.iter().map(|t| t.schema()).collect(),
            model: None,
            max_tokens: None,
        };
        let chat_stream = match runtime.chat.complete(req, ct.clone()).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Step::Done(StopReason::Error(e.to_string()))).await;
                return Ok(());
            }
        };

        let mut text_acc = String::new();
        let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
        let mut usage: Option<Usage> = None;

        let mut s = chat_stream;
        loop {
            tokio::select! {
                _ = ct.cancelled() => {
                    let _ = tx.send(Step::Done(StopReason::Cancelled)).await;
                    bus.publish(BusEvent::Cancelled { session });
                    return Ok(());
                }
                next = s.next() => {
                    let Some(item) = next else { break };
                    match item {
                        Ok(ChatDelta::Text(t)) => {
                            let _ = tx.send(Step::StreamDelta(t.clone())).await;
                            bus.publish(BusEvent::ChatStreamDelta { session, text: t.clone() });
                            text_acc.push_str(&t);
                        }
                        Ok(ChatDelta::Thinking(t)) => {
                            let _ = tx.send(Step::Thinking(t.clone())).await;
                        }
                        Ok(ChatDelta::ToolUse { id, name, input }) => {
                            tool_calls.push((id.clone(), name.clone(), input.clone()));
                        }
                        Ok(ChatDelta::Stop { reason: _, usage: u }) => {
                            usage = u;
                        }
                        Err(e) => {
                            let _ = tx.send(Step::Done(StopReason::Error(e.to_string()))).await;
                            return Ok(());
                        }
                    }
                }
            }
        }

        if let Some(u) = usage.as_ref() {
            budget.record_tokens(u.prompt + u.completion);
            bus.publish(BusEvent::TokenUsage {
                session,
                provider: runtime.chat.name().to_string(),
                model: "(unknown)".to_string(),
                prompt: u.prompt,
                prompt_cached: u.prompt_cached,
                completion: u.completion,
            });
        }

        // Persist assistant message (text + tool_uses).
        let mut content: Vec<ContentBlock> = Vec::new();
        if !text_acc.is_empty() {
            content.push(ContentBlock::Text(text_acc.clone()));
        }
        for (id, name, input) in &tool_calls {
            content.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
        }
        let assistant_msg = Message {
            id: Uuid::new_v4(),
            role: Role::Assistant,
            content,
            created_at_ms: AgentRuntime::now_ms(),
            session,
            metadata: Default::default(),
        };
        let assistant_id = assistant_msg.id;
        let _ = runtime.memory.write(scope.clone(), assistant_msg.clone()).await;
        bus.publish(BusEvent::MemoryWritten {
            session,
            scope: format!("{:?}", scope),
            id: assistant_id,
        });
        messages.push(assistant_msg);

        budget.record_step();
        step_n += 1;
        bus.publish(BusEvent::AgentStepCompleted {
            session,
            step: step_n - 1,
        });

        // ---- Dispatch tool calls (if any) ----
        if !tool_calls.is_empty() {
            let mut tool_result_blocks: Vec<ContentBlock> = Vec::new();
            for (id, name, input) in &tool_calls {
                budget.record_tool_call();
                if let Some(reason) = budget.is_exceeded() {
                    let _ = tx
                        .send(Step::Done(StopReason::BudgetExceeded(reason.to_string())))
                        .await;
                    bus.publish(BusEvent::BudgetExceeded {
                        session,
                        reason: reason.to_string(),
                    });
                    return Ok(());
                }
                let _ = tx
                    .send(Step::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    })
                    .await;
                let started = web_time::Instant::now();
                // ---- Permission gate ----
                let decision = runtime
                    .permission
                    .ask(AskInput {
                        kind: name.clone(),
                        title: format!("{name}({})", input.to_string().chars().take(60).collect::<String>()),
                        locations: extract_paths(input),
                        raw_input: input.clone(),
                    })
                    .await
                    .unwrap_or(PermissionDecision::Reject);
                if decision == PermissionDecision::Reject {
                    let output = serde_json::json!({
                        "error": format!("permission denied: tool {name} was rejected by the user"),
                    });
                    let _ = tx
                        .send(Step::ToolResult {
                            tool_use_id: id.clone(),
                            output: output.clone(),
                            is_error: true,
                        })
                        .await;
                    tool_result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: output.to_string(),
                        is_error: true,
                    });
                    continue;
                }
                let (output, is_error) = match runtime.tool_by_name(name) {
                    Some(t) => {
                        tracing::info!(target: "operon::agent", tool = %name, "tool.invoke");
                        // Per-tool child cancellation token so the UI's
                        // tool-card Cancel button (Phase 3) can abort
                        // a single tool call without killing the whole
                        // turn. Spawning as a child of the outer `ct`
                        // means turn-level Stop still cancels this
                        // child by propagation.
                        let tool_ct = ct.child_token();
                        // Register the handle so `runtime.cancel_tool`
                        // can fire it from anywhere.
                        if let Ok(mut map) = runtime.tool_cancellations.lock() {
                            map.insert(id.clone(), tool_ct.clone());
                        }
                        // Chunk channel: the runtime forwards every
                        // chunk the tool emits as a `Step::ToolChunk`
                        // event with the same `tool_use_id`. Dropped
                        // (and thus the forwarding task finishes) when
                        // `invoke_streaming` returns.
                        let (chunk_tx, mut chunk_rx) =
                            tokio::sync::mpsc::unbounded_channel::<crate::traits::ToolChunk>();
                        let id_for_forward = id.clone();
                        let tx_for_forward = tx.clone();
                        let forward_task = tokio::spawn(async move {
                            while let Some(chunk) = chunk_rx.recv().await {
                                let _ = tx_for_forward
                                    .send(Step::ToolChunk {
                                        tool_use_id: id_for_forward.clone(),
                                        kind: chunk.kind,
                                        bytes: chunk.bytes,
                                    })
                                    .await;
                            }
                        });
                        let res = t
                            .invoke_streaming(input.clone(), tool_ct, chunk_tx)
                            .await;
                        // Wait for the forward task to drain — once
                        // chunk_tx is dropped (right after the await
                        // above) the receiver closes naturally.
                        let _ = forward_task.await;
                        // Deregister the handle: either the tool
                        // completed normally or it was cancelled —
                        // either way the entry is no longer cancellable.
                        if let Ok(mut map) = runtime.tool_cancellations.lock() {
                            map.remove(id);
                        }
                        match res {
                            Ok(v) => (v, false),
                            Err(e) => (serde_json::json!({"error": e.to_string()}), true),
                        }
                    }
                    None => (
                        serde_json::json!({"error": format!("tool not found: {name}")}),
                        true,
                    ),
                };
                bus.publish(BusEvent::ToolInvoked {
                    session,
                    tool: name.clone(),
                    latency_ms: started.elapsed().as_millis() as u64,
                });
                let _ = tx
                    .send(Step::ToolResult {
                        tool_use_id: id.clone(),
                        output: output.clone(),
                        is_error,
                    })
                    .await;
                tool_result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: output.to_string(),
                    is_error,
                });
            }
            // Append tool results as a new message and loop.
            let tool_msg = Message {
                id: Uuid::new_v4(),
                role: Role::Tool,
                content: tool_result_blocks,
                created_at_ms: AgentRuntime::now_ms(),
                session,
                metadata: Default::default(),
            };
            let _ = runtime.memory.write(scope.clone(), tool_msg.clone()).await;
            messages.push(tool_msg);
            // continue loop — chat plugin will be called again.
            continue;
        }

        // No tool calls — finish.
        let _ = tx.send(Step::Done(StopReason::EndTurn)).await;
        return Ok(());
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Best-effort extraction of paths from a tool-call input for the permission UI.
/// Looks at common keys (`path`, `cwd`, `paths[]`).
fn extract_paths(input: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
        out.push(p.to_string());
    }
    if let Some(c) = input.get("cwd").and_then(|v| v.as_str()) {
        out.push(c.to_string());
    }
    if let Some(arr) = input.get("paths").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() {
                out.push(s.to_string());
            }
        }
    }
    out
}
