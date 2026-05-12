//! Slice A1 smoke test — drives a real Anthropic API call through
//! `AgentRuntime` + `AnthropicChatPlugin` and asserts text streams back.
//!
//! Gated `#[ignore]`. Run with:
//!   cargo test -p operon-plugins-anthropic --test live_anthropic -- --ignored --nocapture
//! Requires `ANTHROPIC_API_KEY` in the env or stored under
//! `provider/anthropic/api-key` in the SecretStore.

use operon_core::{
    budget::Budget,
    bus::EventBus,
    memory::InMemoryStore,
    runtime::{AgentRuntime, Step, StopReason},
    secrets::EnvSecretStore,
    traits::{CancellationToken, Scope},
};
use operon_plugins_anthropic::{AnthropicChatPlugin, AnthropicConfig};
use std::sync::Arc;
use tokio_stream::StreamExt;
use uuid::Uuid;

fn have_api_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok()
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live API; set ANTHROPIC_API_KEY and run with --ignored"]
async fn live_anthropic_streams_text() {
    if !have_api_key() {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return;
    }

    let secrets = Arc::new(EnvSecretStore::new("ANTHROPIC_"));
    let cfg = AnthropicConfig {
        // Use a small/fast model so the smoke test stays cheap.
        model: "claude-haiku-4-5-20251001".to_string(),
        max_tokens: 64,
        ..Default::default()
    };
    let chat = Arc::new(
        AnthropicChatPlugin::new(cfg, secrets).expect("plugin builds"),
    );

    let runtime = Arc::new(AgentRuntime::new(
        chat,
        vec![],
        Arc::new(InMemoryStore::new()),
        EventBus::new(64),
    ));

    let session = Uuid::new_v4();
    let ct = CancellationToken::new();
    let mut stream = runtime.clone().run(
        session,
        Scope::User,
        "Reply with exactly the word PING.".to_string(),
        Budget::unlimited(),
        ct,
    );

    let mut got_text = String::new();
    let mut saw_done = false;
    while let Some(step) = stream.next().await {
        match step {
            Step::StreamDelta(t) => got_text.push_str(&t),
            Step::Done(StopReason::EndTurn) => {
                saw_done = true;
                break;
            }
            Step::Done(other) => panic!("unexpected stop: {other:?}"),
            _ => {}
        }
    }

    assert!(saw_done, "agent loop never emitted Done(EndTurn)");
    assert!(
        got_text.to_uppercase().contains("PING"),
        "expected response to mention PING, got: {got_text:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live API; set ANTHROPIC_API_KEY and run with --ignored"]
async fn live_anthropic_extended_thinking_streams_thinking_steps() {
    if !have_api_key() {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return;
    }
    // Slice A1 doesn't yet enable extended thinking on the request body — the
    // mapping from `thinking_delta` SSE → `Step::Thinking` is wired and tested
    // by unit tests in operon-plugins-anthropic. This test is a placeholder
    // for when the AnthropicConfig grows a `thinking_budget_tokens: Option<u32>`
    // knob (Slice A1.1 follow-up).
    eprintln!("skipping: extended thinking request param not yet wired (Slice A1.1)");
}
