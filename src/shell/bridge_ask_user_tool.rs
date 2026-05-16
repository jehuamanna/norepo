//! `operon_ask_user` MCP tool — out-of-process counterpart to the
//! in-process [`crate::shell::bridge_ask_user_executor::BridgeAskUserExecutor`].
//!
//! In chat mode the executor lives inside the same Rust process as
//! the chat plugin's stdio bridge, so the tool result hops through
//! one in-process channel. Terminal-mode Claude is a PTY child that
//! we don't intercept — it spawns its own MCP servers via
//! `.mcp.json`. This handler is what `operon-mcp` (the bridge stub
//! binary) routes `tools/call ask_user` requests to.
//!
//! Wire shape stays identical to the chat-mode tool so the model
//! sees the same `{questions, answers}` payload regardless of which
//! companion surface raised the picker. That makes the picker UI
//! and the model's downstream reasoning agnostic to transport.

#![cfg(all(unix, not(target_arch = "wasm32")))]

use async_trait::async_trait;
use operon_bridge::{ToolHandler, ToolHandlerError};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::local_mode::bridge_runtime::{BridgeUiCommand, BridgeUiSender};
use crate::shell::companion_state::{park_ask_user_responder, AskUserPromptEntry};

/// Tool handler. The picker state lives in
/// `companion_state::ASK_USER_PROMPTS` (a Dioxus `GlobalSignal`),
/// which can only be written from a thread with a Dioxus runtime
/// guard installed. We hold a `BridgeUiSender` and post a
/// [`BridgeUiCommand::PushAskUserPrompt`] command instead of writing
/// the signal directly — the Dioxus-side drain task applies it.
pub struct OperonAskUserTool {
    ui: BridgeUiSender,
}

impl OperonAskUserTool {
    pub fn new(ui: BridgeUiSender) -> Self {
        Self { ui }
    }
}

#[async_trait]
impl ToolHandler for OperonAskUserTool {
    fn name(&self) -> &str {
        // Surfaces to Claude as `mcp__operon__ask_user` once the
        // bridge advertises itself under the `operon` server name in
        // the per-spawn `.mcp.json`. Matches the chat-mode tool name
        // exactly so prompts the model has seen in either mode use
        // the same call shape.
        "ask_user"
    }

    fn description(&self) -> &str {
        // Same prose as the chat-mode tool (see
        // `crates/operon-plugins-claude-code/src/permission_bridge.rs`
        // around line 421) so the model's selection criteria are
        // unchanged. If the description ever diverges from the chat-
        // mode one, expect transport-dependent picker frequency.
        "Ask the user a clarifying question with structured options. Use this whenever you would normally use the built-in AskUserQuestion tool — that one is disabled here, so this is the only way to surface a picker. Input shape mirrors AskUserQuestion exactly. The response will be `{questions, answers}` where `answers` maps each question text to the chosen option label (string for single-select, array for multiSelect)."
    }

    fn input_schema(&self) -> Value {
        // Verbatim copy of the chat-mode bridge's schema so any
        // schema-aware tooling on the model's side (the SDK
        // generates type hints from this) sees the same shape on
        // both transports. Keep these two in sync if you ever
        // evolve the schema.
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question":    { "type": "string", "description": "The complete question. Should end with a question mark." },
                            "header":      { "type": "string", "description": "Very short label (max ~12 chars)." },
                            "multiSelect": { "type": "boolean", "description": "If true, allow multiple selections. Defaults to false." },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label":       { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label", "description"]
                                }
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn call(&self, args: Value) -> Result<Value, ToolHandlerError> {
        // Same input validation the in-process executor enforces.
        // An empty or missing array would render a useless picker
        // and is almost always a model bug — failing loud lets the
        // model self-correct on the next turn.
        let questions = args.get("questions").cloned().unwrap_or(Value::Null);
        match &questions {
            Value::Array(a) if !a.is_empty() => {}
            _ => {
                return Err(ToolHandlerError::new(
                    "ask_user requires non-empty `questions` array",
                ));
            }
        }

        let prompt_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel::<Option<Value>>();
        // park_ask_user_responder writes to a OnceLock<Mutex<HashMap>>
        // (NOT a Dioxus signal) so it's safe to call from the bridge
        // thread directly. Only the `push_ask_user_prompt` call,
        // which writes the ASK_USER_PROMPTS GlobalSignal, has to go
        // through the channel.
        park_ask_user_responder(prompt_id.clone(), tx);

        // Terminal-mode picker has no chat session to attribute the
        // prompt to, so source_session is None. The chat-side path
        // populates these for context in the prompt card; we leave
        // them empty for now and surface "terminal" affordances in a
        // later milestone.
        self.ui.send(BridgeUiCommand::PushAskUserPrompt(AskUserPromptEntry {
            id: prompt_id.clone(),
            questions: questions.clone(),
            source_session: None,
            source_cwd: None,
            created_at: std::time::SystemTime::now(),
        }));

        match rx.await {
            Ok(Some(answers)) => {
                // MCP `content` is a Vec of typed blocks. We encode
                // the structured `{questions, answers}` payload as a
                // single text block — the same shape claude expects
                // and the same shape chat-mode returns (where the
                // wrapping happens inside the permission_bridge
                // rather than here).
                let payload = json!({ "questions": questions, "answers": answers });
                Ok(json!([{ "type": "text", "text": payload.to_string() }]))
            }
            Ok(None) => Err(ToolHandlerError::new("user cancelled the question prompt")),
            Err(_) => Err(ToolHandlerError::new(
                "ask_user responder dropped before user answered",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    // Same constraint as `bridge_ask_user_executor::tests`: anything
    // that exercises the picker flow needs a live Dioxus runtime
    // (`ASK_USER_PROMPTS` is a `GlobalSignal`). Only the
    // input-validation paths can run without that.
    use super::*;
    use crate::local_mode::bridge_runtime::make_ui_channel;

    fn stub_tool() -> OperonAskUserTool {
        // Drop the receiver — these validation-path tests bail
        // before any `self.ui.send(...)` runs.
        let (tx, _rx) = make_ui_channel();
        OperonAskUserTool::new(tx)
    }

    #[tokio::test]
    async fn errors_when_questions_missing() {
        let res = stub_tool().call(json!({})).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn errors_when_questions_is_empty_array() {
        let res = stub_tool().call(json!({ "questions": [] })).await;
        assert!(res.is_err());
    }

    #[test]
    fn input_schema_matches_chat_mode_shape() {
        // If this drifts from the chat-mode schema (see
        // `permission_bridge.rs::tools/list` around `ask_user`), the
        // model will get different schemas on different transports.
        let tool = stub_tool();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["questions"]));
        assert_eq!(schema["properties"]["questions"]["type"], "array");
        assert_eq!(
            schema["properties"]["questions"]["items"]["required"],
            json!(["question", "header", "options"])
        );
    }
}
