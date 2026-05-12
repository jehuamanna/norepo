//! Agent-backend picker dropdown (Slice A12).
//!
//! Surfaces the available backends so the user can switch between
//! claude-code (subprocess) and the new in-process runtime per project /
//! per session. The actual backend swap is done by the caller — this
//! component is the UI; it fires `on_change` with the new selection and
//! the caller is responsible for replacing the `Arc<dyn AgentBackend>`
//! used by cascade / executor.
//!
//! Mirrors the styling of the existing model & permission-mode pickers
//! in `companion_chat.rs` so the toolbar stays visually consistent.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentBackendKind {
    /// Subprocess to the `claude` CLI (legacy default).
    ClaudeCode,
    /// In-process Rust runtime (Anthropic / OpenAI / Google / Local).
    Runtime,
}

impl AgentBackendKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Runtime => "runtime",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Runtime => "Runtime",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(Self::ClaudeCode),
            "runtime" => Some(Self::Runtime),
            _ => None,
        }
    }

    /// Default ordering for the dropdown; claude-code first to preserve
    /// the legacy first-choice behaviour while we ramp up the runtime.
    pub fn all() -> [AgentBackendKind; 2] {
        [Self::ClaudeCode, Self::Runtime]
    }
}

#[derive(Clone, Debug, PartialEq, Props)]
pub struct AgentBackendPickerProps {
    /// Currently-selected backend.
    pub current: AgentBackendKind,
    /// Whether the picker is interactive. Set to `false` while a turn is
    /// in flight — switching mid-turn invalidates the session binding.
    #[props(default = true)]
    pub enabled: bool,
    /// Fires when the user picks a different backend.
    pub on_change: EventHandler<AgentBackendKind>,
}

#[component]
pub fn AgentBackendPicker(props: AgentBackendPickerProps) -> Element {
    let current = props.current;
    let enabled = props.enabled;
    let on_change = props.on_change;
    rsx! {
        label {
            class: "operon-companion-toolbar-label",
            title: "Agent backend",
            span { class: "sr-only", "Agent backend" }
            select {
                class: "operon-companion-model-picker",
                "data-testid": "agent-backend-picker",
                disabled: !enabled,
                onchange: move |e| {
                    if let Some(k) = AgentBackendKind::parse(&e.value()) {
                        on_change.call(k);
                    }
                },
                for kind in AgentBackendKind::all().iter() {
                    option {
                        value: "{kind.id()}",
                        selected: *kind == current,
                        "{kind.label()}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_ids_round_trip_label() {
        let cc = AgentBackendKind::parse("claude-code").unwrap();
        assert_eq!(cc.id(), "claude-code");
        assert_eq!(cc.label(), "Claude Code");
        let rt = AgentBackendKind::parse("runtime").unwrap();
        assert_eq!(rt.id(), "runtime");
        assert_eq!(rt.label(), "Runtime");
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(AgentBackendKind::parse("magic").is_none());
        assert!(AgentBackendKind::parse("").is_none());
    }

    #[test]
    fn all_lists_both_backends_with_claude_code_first() {
        let all = AgentBackendKind::all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], AgentBackendKind::ClaudeCode);
        assert_eq!(all[1], AgentBackendKind::Runtime);
    }
}
