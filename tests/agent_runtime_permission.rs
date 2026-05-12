//! Integration tests for the new permission gate (Slice A3) wired into
//! `AgentRuntime`. Uses `EchoChatPlugin` + `EchoToolPlugin` so no LLM
//! and no real shell runs.

#![cfg(not(target_arch = "wasm32"))]

use futures::StreamExt;
use operon_dioxus::agent::permission::{
    AskInput, DefaultDecision, PermissionDecision, PermissionGate, PermissionRule, RuleSet,
};
use operon_dioxus::agent::plugins::{EchoChatPlugin, EchoToolPlugin};
use operon_dioxus::agent::runtime::{AgentRuntime, Step, StopReason};
use operon_dioxus::agent::traits::{ChatPlugin, MemoryPlugin, ToolPlugin};
use operon_dioxus::agent::{Budget, CancellationToken, EventBus, InMemoryStore, Scope};
use std::sync::Arc;
use uuid::Uuid;

fn build_runtime(gate: PermissionGate) -> Arc<AgentRuntime> {
    let chat = Arc::new(EchoChatPlugin::new(
        "echo-chat",
        vec![
            EchoChatPlugin::turn_with_tool(
                "I'll call the echo tool",
                "tool-1",
                "echo",
                serde_json::json!({"a": 1}),
            ),
            // After receiving the tool result the model finishes.
            EchoChatPlugin::turn_done("done"),
        ],
    ));
    let tool: Arc<dyn ToolPlugin> = Arc::new(EchoToolPlugin::new("echo"));
    let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
    let bus = EventBus::new(64);
    Arc::new(
        AgentRuntime::new(chat as Arc<dyn ChatPlugin>, vec![tool], memory, bus)
            .with_permission(gate),
    )
}

async fn run_to_done(runtime: Arc<AgentRuntime>) -> Vec<Step> {
    let mut stream = runtime.run(
        Uuid::new_v4(),
        Scope::User,
        "ping".to_string(),
        Budget::unlimited(),
        CancellationToken::new(),
    );
    let mut steps = Vec::new();
    while let Some(s) = stream.next().await {
        steps.push(s.clone());
        if matches!(s, Step::Done(_)) {
            break;
        }
    }
    steps
}

#[tokio::test]
async fn default_gate_lets_tool_call_through() {
    // Default `PermissionGate` auto-allows — preserves legacy behaviour.
    let steps = run_to_done(build_runtime(PermissionGate::default())).await;
    assert!(matches!(steps.last(), Some(Step::Done(StopReason::EndTurn))));
    let tool_results: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s, Step::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 1);
    if let Step::ToolResult { is_error, .. } = tool_results[0] {
        assert!(!is_error, "default gate should allow the tool call");
    }
}

#[tokio::test]
async fn strict_gate_rejects_unconfigured_tool() {
    let steps = run_to_done(build_runtime(PermissionGate::strict())).await;
    // Loop still terminates (the runtime feeds the rejection back to the
    // model and the next turn ends).
    assert!(matches!(steps.last(), Some(Step::Done(StopReason::EndTurn))));
    // The tool result should be marked is_error=true with a permission-denied message.
    let tool_results: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s, Step::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 1);
    if let Step::ToolResult { output, is_error, .. } = tool_results[0] {
        assert!(is_error, "strict gate should reject the tool call");
        let body = output.to_string();
        assert!(body.contains("permission denied"), "got: {body}");
    }
}

#[tokio::test]
async fn rule_allowlist_unblocks_specific_tool() {
    let mut rules = RuleSet::default();
    rules.rules.push(PermissionRule {
        pattern: "echo".into(),
        action: PermissionDecision::AllowOnce,
    });
    // Default-deny everything else; rule explicitly allows `echo`.
    let gate = PermissionGate::new(rules).with_default(DefaultDecision::Reject);
    let steps = run_to_done(build_runtime(gate)).await;
    let tool_results: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s, Step::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 1);
    if let Step::ToolResult { is_error, .. } = tool_results[0] {
        assert!(!is_error, "rule allowlist should permit echo");
    }
}

#[tokio::test]
async fn sticky_allow_unblocks_after_one_match() {
    let gate = PermissionGate::strict();
    gate.allow_always("echo");
    let steps = run_to_done(build_runtime(gate)).await;
    let tool_results: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s, Step::ToolResult { .. }))
        .collect();
    if let Step::ToolResult { is_error, .. } = tool_results[0] {
        assert!(!is_error, "sticky allow should permit echo");
    }
}

#[tokio::test]
async fn ask_input_kind_is_tool_name() {
    // Validates the contract the runtime hands the gate: `kind` carries the
    // tool name so wildcard rules can target tool families.
    let gate = PermissionGate::default();
    let d = gate
        .ask(AskInput {
            kind: "echo".into(),
            title: "echo".into(),
            locations: vec![],
            raw_input: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert_eq!(d, PermissionDecision::AllowOnce);
}
