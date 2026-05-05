//! Companion chat surface — minimal Plans-Phase-4 implementation.
//!
//! Wires `EchoChatPlugin` (no API key required) into a single-turn chat UI that
//! drives `AgentRuntime` and renders the resulting `Step` stream into the
//! Companion Area.
//!
//! Deferred to follow-up: memory inspector, plugin manager, MCP grant modal,
//! AnthropicChatPlugin wiring, model picker, stream cancellation tests in
//! Playwright.

use dioxus::prelude::*;
use futures::StreamExt;
use std::sync::Arc;
use uuid::Uuid;

use crate::agent::plugins::{EchoChatPlugin, EchoToolPlugin};
use crate::agent::runtime::{AgentRuntime, Step, StopReason};
use crate::agent::traits::{ChatPlugin, MemoryPlugin, ToolPlugin};
use crate::agent::{Budget, CancellationToken, EventBus, InMemoryStore, Scope};

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayMessage {
    pub role: DisplayRole,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayRole {
    User,
    Assistant,
    System,
}

#[component]
pub fn CompanionChat() -> Element {
    let messages = use_signal::<Vec<DisplayMessage>>(Vec::new);
    let mut composer = use_signal(String::new);
    let in_flight = use_signal(|| false);
    let active_ct = use_signal::<Option<CancellationToken>>(|| None);

    rsx! {
        div { class: "operon-companion-chat",
            "data-region": "companion-chat",
            div { class: "operon-companion-chat-header",
                span { class: "operon-companion-chat-title", "Companion" }
                if *in_flight.read() {
                    button {
                        class: "operon-companion-chat-stop",
                        "data-testid": "companion-stop",
                        onclick: move |_| {
                            if let Some(ct) = active_ct.read().as_ref() {
                                ct.cancel();
                            }
                        },
                        "Stop"
                    }
                }
            }
            div { class: "operon-companion-chat-transcript",
                "data-testid": "companion-transcript",
                for (i, msg) in messages.read().iter().enumerate() {
                    div {
                        key: "{i}",
                        class: match msg.role {
                            DisplayRole::User => "operon-companion-msg operon-companion-msg-user",
                            DisplayRole::Assistant => "operon-companion-msg operon-companion-msg-assistant",
                            DisplayRole::System => "operon-companion-msg operon-companion-msg-system",
                        },
                        "data-role": match msg.role {
                            DisplayRole::User => "user",
                            DisplayRole::Assistant => "assistant",
                            DisplayRole::System => "system",
                        },
                        "{msg.text}"
                    }
                }
            }
            div { class: "operon-companion-chat-composer",
                textarea {
                    class: "operon-companion-chat-input",
                    "data-testid": "companion-input",
                    value: "{composer}",
                    placeholder: "Type a message... (Cmd/Ctrl+Enter to send)",
                    oninput: move |e| composer.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter && (e.modifiers().ctrl() || e.modifiers().meta()) {
                            run_turn(messages, composer, in_flight, active_ct);
                        }
                    },
                }
                button {
                    class: "operon-companion-chat-send",
                    "data-testid": "companion-send",
                    disabled: *in_flight.read(),
                    onclick: move |_| run_turn(messages, composer, in_flight, active_ct),
                    "Send"
                }
            }
        }
    }
}

/// Take the current composer text, append it to the transcript, spawn the
/// agent loop, and stream `Step`s into the transcript signal.
fn run_turn(
    mut messages: Signal<Vec<DisplayMessage>>,
    mut composer: Signal<String>,
    mut in_flight: Signal<bool>,
    mut active_ct: Signal<Option<CancellationToken>>,
) {
    if *in_flight.read() {
        return;
    }
    let text = composer.read().trim().to_string();
    if text.is_empty() {
        return;
    }
    composer.set(String::new());
    messages.write().push(DisplayMessage {
        role: DisplayRole::User,
        text: text.clone(),
    });
    messages.write().push(DisplayMessage {
        role: DisplayRole::Assistant,
        text: String::new(),
    });
    in_flight.set(true);
    let ct = CancellationToken::new();
    active_ct.set(Some(ct.clone()));

    spawn(async move {
        let reply = format!("echo: {text}");
        let chat: Arc<dyn ChatPlugin> = Arc::new(EchoChatPlugin::new(
            "echo-companion",
            vec![EchoChatPlugin::turn_done(&reply)],
        ));
        let tool: Arc<dyn ToolPlugin> = Arc::new(EchoToolPlugin::new("echo"));
        let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
        let bus = EventBus::new(64);
        let runtime = Arc::new(AgentRuntime::new(chat, vec![tool], memory, bus));
        let session = Uuid::new_v4();
        let mut stream = runtime.run(
            session,
            Scope::User,
            text,
            Budget::unlimited(),
            ct,
        );
        while let Some(step) = stream.next().await {
            match step {
                Step::StreamDelta(t) => {
                    let mut m = messages.write();
                    if let Some(last) = m.last_mut() {
                        if last.role == DisplayRole::Assistant {
                            last.text.push_str(&t);
                        }
                    }
                }
                Step::Done(reason) => {
                    if let StopReason::Cancelled = reason {
                        messages.write().push(DisplayMessage {
                            role: DisplayRole::System,
                            text: "(cancelled)".into(),
                        });
                    } else if let StopReason::Error(e) = reason {
                        messages.write().push(DisplayMessage {
                            role: DisplayRole::System,
                            text: format!("error: {e}"),
                        });
                    }
                    break;
                }
                _ => {}
            }
        }
        in_flight.set(false);
        active_ct.set(None);
    });
}
