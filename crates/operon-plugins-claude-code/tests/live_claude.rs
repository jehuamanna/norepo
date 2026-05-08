//! Live integration test that drives the real `claude` CLI through
//! `ClaudeCodeChatPlugin`. Skipped unless `OPERON_CLAUDE_BIN` is set so CI
//! and offline checkouts don't need the binary installed.

#![cfg(not(target_arch = "wasm32"))]

use futures::StreamExt;
use operon_core::traits::{
    CancellationToken, ChatDelta, ChatPlugin, ChatRequest, ContentBlock, Message, Role,
};
use operon_plugins_claude_code::{ClaudeCodeChatPlugin, ClaudeCodeConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

fn maybe_bin() -> Option<PathBuf> {
    std::env::var_os("OPERON_CLAUDE_BIN").map(PathBuf::from)
}

fn user_msg(text: &str, session: Uuid) -> Message {
    Message {
        id: Uuid::new_v4(),
        role: Role::User,
        content: vec![ContentBlock::Text(text.into())],
        created_at_ms: 0,
        session,
        metadata: HashMap::new(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_claude_responds_in_bound_repo() {
    let Some(bin) = maybe_bin() else {
        eprintln!("skipping: set OPERON_CLAUDE_BIN to run");
        return;
    };
    if !bin.exists() {
        eprintln!("skipping: OPERON_CLAUDE_BIN={} does not exist", bin.display());
        return;
    }

    let plugin = Arc::new(ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
        claude_bin: bin,
        model: None,
    }));

    let tmp = tempfile::tempdir().expect("tempdir");
    let session = Uuid::new_v4();
    plugin.bind_session(session, tmp.path().to_path_buf());

    let req = ChatRequest {
        system: None,
        messages: vec![user_msg(
            "Reply with exactly the single word: pong",
            session,
        )],
        tools: vec![],
        model: None,
        max_tokens: None,
    };
    let ct = CancellationToken::new();

    let mut stream = plugin
        .complete(req, ct)
        .await
        .expect("complete starts");

    let mut text = String::new();
    let mut saw_stop = false;
    while let Some(item) = stream.next().await {
        match item.expect("stream item is Ok") {
            ChatDelta::Text(t) => text.push_str(&t),
            ChatDelta::Stop { .. } => {
                saw_stop = true;
                break;
            }
            ChatDelta::ToolUse { .. } => {}
        }
    }
    assert!(saw_stop, "expected Stop delta");
    assert!(
        text.to_lowercase().contains("pong"),
        "expected response to contain 'pong', got: {text:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_claude_errors_when_session_not_bound() {
    let Some(bin) = maybe_bin() else {
        eprintln!("skipping: set OPERON_CLAUDE_BIN to run");
        return;
    };
    let plugin = Arc::new(ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
        claude_bin: bin,
        model: None,
    }));
    let session = Uuid::new_v4();
    // Intentionally do NOT call bind_session.
    let req = ChatRequest {
        system: None,
        messages: vec![user_msg("hi", session)],
        tools: vec![],
        model: None,
        max_tokens: None,
    };
    let err = plugin
        .complete(req, CancellationToken::new())
        .await
        .err()
        .expect("complete errors when session has no binding");
    let msg = format!("{err}");
    assert!(
        msg.contains("not bound") || msg.contains("repository"),
        "error should mention missing binding, got {msg}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_claude_two_sessions_share_no_state() {
    // Two parallel Operon sessions, each with its own cwd. After turn 1,
    // each binding caches its own claude_session_id; bindings are isolated.
    let Some(bin) = maybe_bin() else {
        eprintln!("skipping: set OPERON_CLAUDE_BIN to run");
        return;
    };
    if !bin.exists() {
        return;
    }
    let plugin = Arc::new(ClaudeCodeChatPlugin::new(ClaudeCodeConfig {
        claude_bin: bin,
        model: None,
    }));

    let tmp_a = tempfile::tempdir().expect("tempdir A");
    let tmp_b = tempfile::tempdir().expect("tempdir B");
    let s_a = Uuid::new_v4();
    let s_b = Uuid::new_v4();
    plugin.bind_session(s_a, tmp_a.path().to_path_buf());
    plugin.bind_session(s_b, tmp_b.path().to_path_buf());

    for (sid, label) in [(s_a, "alpha"), (s_b, "bravo")] {
        let req = ChatRequest {
            system: None,
            messages: vec![user_msg(
                &format!("Reply with the single word: {label}"),
                sid,
            )],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let mut stream = plugin
            .complete(req, CancellationToken::new())
            .await
            .expect("complete starts");
        while let Some(item) = stream.next().await {
            match item.expect("ok") {
                operon_core::traits::ChatDelta::Stop { .. } => break,
                _ => {}
            }
        }
    }

    let id_a = plugin
        .current_claude_session(s_a)
        .expect("session A should have a claude id after a turn");
    let id_b = plugin
        .current_claude_session(s_b)
        .expect("session B should have a claude id after a turn");
    assert_ne!(id_a, id_b, "two operon sessions must map to two claude sessions");
}
