//! Integration tests for AgentRuntime (Plans-Phase-2).
//! Driven through `EchoChatPlugin` + `EchoToolPlugin` so no LLM/network is involved.

#![cfg(not(target_arch = "wasm32"))]

use futures::StreamExt;
use operon_dioxus::agent::{
    Budget, BusEvent, CancellationToken, EventBus, InMemoryStore, OperonError, Scope,
};
use operon_dioxus::agent::plugins::{EchoChatPlugin, EchoToolPlugin};
use operon_dioxus::agent::runtime::{AgentRuntime, Step, StopReason};
use operon_dioxus::agent::traits::{ChatPlugin, ChatRequest, MemoryPlugin, ToolPlugin};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn three_step_echo_loop_completes() {
    let chat = Arc::new(EchoChatPlugin::new(
        "echo-chat",
        vec![
            EchoChatPlugin::turn_with_tool(
                "thinking…",
                "tool-1",
                "echo",
                serde_json::json!({"a": 1}),
            ),
            EchoChatPlugin::turn_with_tool(
                "still thinking…",
                "tool-2",
                "echo",
                serde_json::json!({"b": 2}),
            ),
            EchoChatPlugin::turn_done("done"),
        ],
    ));
    let tool: Arc<dyn ToolPlugin> = Arc::new(EchoToolPlugin::new("echo"));
    let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
    let bus = EventBus::new(64);
    let runtime = Arc::new(AgentRuntime::new(
        chat as Arc<dyn ChatPlugin>,
        vec![tool],
        memory.clone(),
        bus.clone(),
    ));

    let session = Uuid::new_v4();
    let mut stream = runtime.run(
        session,
        Scope::Project(Uuid::new_v4()),
        "hello".to_string(),
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

    assert!(matches!(steps.first(), Some(Step::Started)));
    assert!(matches!(steps.last(), Some(Step::Done(StopReason::EndTurn))));
    assert!(steps.iter().any(|s| matches!(s, Step::ToolCall { .. })));
    assert!(steps.iter().any(|s| matches!(s, Step::ToolResult { .. })));
}

#[tokio::test]
async fn budget_exhaustion_terminates_loop() {
    let chat = Arc::new(EchoChatPlugin::new(
        "echo",
        vec![
            EchoChatPlugin::turn_with_tool("…", "t-1", "echo", serde_json::json!({})),
            EchoChatPlugin::turn_with_tool("…", "t-2", "echo", serde_json::json!({})),
            EchoChatPlugin::turn_done("never reached"),
        ],
    ));
    let tool: Arc<dyn ToolPlugin> = Arc::new(EchoToolPlugin::new("echo"));
    let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
    let bus = EventBus::new(64);
    let runtime = Arc::new(AgentRuntime::new(
        chat as Arc<dyn ChatPlugin>,
        vec![tool],
        memory,
        bus.clone(),
    ));

    let session = Uuid::new_v4();
    let mut rx = bus.subscribe();
    let mut stream = runtime.run(
        session,
        Scope::User,
        "test".into(),
        Budget::new(None, None, Some(1), None), // max 1 tool call
        CancellationToken::new(),
    );
    while let Some(s) = stream.next().await {
        if matches!(s, Step::Done(_)) {
            break;
        }
    }

    let mut saw_budget = false;
    while let Ok(ev) = rx.try_recv() {
        if let BusEvent::BudgetExceeded { reason, .. } = ev {
            assert_eq!(reason, "max_tool_calls");
            saw_budget = true;
        }
    }
    assert!(saw_budget, "expected BudgetExceeded event on bus");
}

#[tokio::test]
async fn cancellation_drops_cleanly() {
    let chat = Arc::new(EchoChatPlugin::new(
        "echo",
        vec![EchoChatPlugin::turn_done("never starts streaming")],
    ));
    let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
    let bus = EventBus::new(8);
    let runtime = Arc::new(AgentRuntime::new(
        chat as Arc<dyn ChatPlugin>,
        vec![],
        memory,
        bus.clone(),
    ));
    let ct = CancellationToken::new();
    ct.cancel(); // pre-cancelled
    let session = Uuid::new_v4();
    let started = std::time::Instant::now();
    let mut stream = runtime.run(
        session,
        Scope::User,
        "test".into(),
        Budget::unlimited(),
        ct,
    );
    let mut saw_cancelled = false;
    while let Some(s) = stream.next().await {
        if let Step::Done(StopReason::Cancelled) = s {
            saw_cancelled = true;
            break;
        }
        if matches!(s, Step::Done(_)) {
            break;
        }
    }
    assert!(saw_cancelled, "expected Step::Done(Cancelled)");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "cancellation took too long: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn memory_writeback_observable() {
    let chat = Arc::new(EchoChatPlugin::new(
        "echo",
        vec![EchoChatPlugin::turn_done("hi back")],
    ));
    let memory = Arc::new(InMemoryStore::new());
    let bus = EventBus::new(64);
    let runtime = Arc::new(AgentRuntime::new(
        chat as Arc<dyn ChatPlugin>,
        vec![],
        memory.clone(),
        bus,
    ));
    let scope = Scope::Project(Uuid::new_v4());
    let session = Uuid::new_v4();
    let mut stream = runtime.run(
        session,
        scope.clone(),
        "hello".into(),
        Budget::unlimited(),
        CancellationToken::new(),
    );
    while let Some(s) = stream.next().await {
        if matches!(s, Step::Done(_)) {
            break;
        }
    }
    let hits = memory.search(scope, "hello", 10).await.unwrap();
    assert!(!hits.is_empty(), "user prompt should be searchable in memory");
}

#[tokio::test]
async fn echo_tool_invoke_returns_input() {
    let t = EchoToolPlugin::new("echo");
    let r = t
        .invoke(serde_json::json!({"x": 42}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r, serde_json::json!({"x": 42}));
}

#[tokio::test]
async fn echo_chat_script_exhausted() {
    let p = EchoChatPlugin::new("e", vec![]);
    let req = ChatRequest {
        system: None,
        messages: vec![],
        tools: vec![],
        model: None,
        max_tokens: None,
    };
    let r = p.complete(req, CancellationToken::new()).await;
    assert!(matches!(r, Err(OperonError::Provider { .. })));
}
