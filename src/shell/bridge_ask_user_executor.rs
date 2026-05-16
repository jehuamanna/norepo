//! Host-side implementation of [`AskUserExecutor`]: turn a
//! `mcp__operon__ask_user` tool call from Claude into an interactive
//! picker rendered in the companion chat, then ship the user's answer
//! back to Claude.
//!
//! Why this exists: the harness-owned `AskUserQuestion` built-in is
//! disabled here (`--disallowedTools AskUserQuestion`) because the
//! harness intercepts its tool_result frames in non-TUI mode and
//! auto-synthesises empty answers regardless of what the host writes
//! on stdin. The only working channel for structured questions is a
//! custom MCP tool whose result the harness passes through verbatim —
//! that's what this executor backs.
//!
//! Flow:
//!   1. Bridge calls `ask(args)` with the verbatim AskUserQuestion-
//!      shaped input.
//!   2. The executor parks a one-shot responder in
//!      `ask_user_responders` and pushes an
//!      [`AskUserPromptEntry`] onto `ASK_USER_PROMPTS` so the
//!      picker card in the chat surface renders it.
//!   3. The picker's Submit handler calls
//!      `submit_ask_user_answers(id, answers)`, which resolves the
//!      one-shot with `Some(answers)`.
//!   4. We wrap `{ questions: <orig>, answers: <map> }` and return —
//!      matching the harness's built-in payload shape so the model's
//!      downstream reasoning is unchanged.
//!
//! Cancellation: the picker can resolve with `None`, or the responder
//! can be dropped without sending (e.g. session torn down). Both
//! surface as a tool-use error to Claude so the model can recover.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use futures::future::BoxFuture;
use operon_core::error::{OperonError, OperonResult};
use operon_plugins_claude_code::AskUserExecutor;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::shell::companion_state::{
    dispatch_push_ask_user_prompt, park_ask_user_responder, AskUserPromptEntry,
};

pub struct BridgeAskUserExecutor {
    session_id: Uuid,
    source_cwd: PathBuf,
}

impl BridgeAskUserExecutor {
    pub fn new(session_id: Uuid, source_cwd: PathBuf) -> Self {
        Self {
            session_id,
            source_cwd,
        }
    }
}

impl AskUserExecutor for BridgeAskUserExecutor {
    fn ask<'a>(&'a self, args: Value) -> BoxFuture<'a, OperonResult<Value>> {
        let session_id = self.session_id;
        let source_cwd = self.source_cwd.clone();
        Box::pin(async move { ask_inner(session_id, source_cwd, args).await })
    }
}

fn err(message: impl Into<String>) -> OperonError {
    OperonError::Plugin {
        plugin: "ask_user".into(),
        source: Box::new(std::io::Error::other(message.into())),
    }
}

async fn ask_inner(
    session_id: Uuid,
    source_cwd: PathBuf,
    args: Value,
) -> OperonResult<Value> {
    // Validate: `questions` must be a non-empty array. The picker
    // can't render anything useful otherwise, and an empty call from
    // the model is almost certainly a bug — fail loudly so Claude
    // sees a tool_use_error and can adapt.
    let questions = args.get("questions").cloned().unwrap_or(Value::Null);
    match &questions {
        Value::Array(a) if !a.is_empty() => {}
        _ => return Err(err("ask_user requires non-empty `questions` array")),
    }

    let prompt_id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel::<Option<Value>>();
    park_ask_user_responder(prompt_id.clone(), tx);

    dispatch_push_ask_user_prompt(AskUserPromptEntry {
        id: prompt_id.clone(),
        questions: questions.clone(),
        source_session: Some(session_id),
        source_cwd: Some(source_cwd),
        created_at: std::time::SystemTime::now(),
    });

    // Wait for the picker. The responder closes (Err) when the
    // session tears down without an answer; treat that the same as
    // an explicit cancel.
    let result = match rx.await {
        Ok(Some(answers)) => Ok(json!({
            "questions": questions,
            "answers": answers,
        })),
        Ok(None) => Err(err("user cancelled the question prompt")),
        Err(_) => Err(err("ask_user responder dropped before user answered")),
    };

    result
}

#[cfg(test)]
mod tests {
    // Note: integration tests that exercise the picker flow are not
    // included here because `ASK_USER_PROMPTS` is a `GlobalSignal`
    // that requires a live Dioxus runtime — those run manually
    // end-to-end against the running app. The unit tests below cover
    // the input-validation paths that don't touch the global state.

    use super::*;

    #[tokio::test]
    async fn returns_error_when_questions_missing() {
        let exec = BridgeAskUserExecutor::new(Uuid::new_v4(), PathBuf::from("/tmp"));
        let res = exec.ask(json!({})).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn returns_error_when_questions_is_empty_array() {
        let exec = BridgeAskUserExecutor::new(Uuid::new_v4(), PathBuf::from("/tmp"));
        let res = exec.ask(json!({ "questions": [] })).await;
        assert!(res.is_err());
    }
}
