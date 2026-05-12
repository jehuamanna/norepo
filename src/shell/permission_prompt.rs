//! Backend-agnostic inline permission prompt component (Slice A12).
//!
//! When the agent backend (claude-code, the new in-process runtime, or
//! any future addition) needs the user to authorise a privileged tool
//! call, it surfaces an `AgentEvent::PermissionRequest`. This component
//! renders that request as a card with three buttons —
//! `Allow once` / `Always allow` / `Reject` — in the chat transcript at
//! the position where the request arrived.
//!
//! On click, `on_decision` fires with the user's choice. The caller is
//! responsible for routing that choice back to the right backend
//! (`PermissionGate::reply` for the runtime, `PermissionBridge` for
//! claude-code). On `AlwaysAllow`, the caller should also call
//! `permission_persist::append_allow_rule` so the same tool doesn't
//! re-prompt on the next turn.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Props)]
pub struct PermissionPromptProps {
    /// Stable id matching the originating request — the caller uses it
    /// to route the reply back to the right backend.
    pub id: String,
    /// Short tag used for matching in the rule engine (e.g. `shell`,
    /// `git:commit`, `file_write`).
    pub kind: String,
    /// Human-readable title to show in the card header.
    pub title: String,
    /// Optional file paths the request touches; rendered as a small list.
    #[props(default = Vec::new())]
    pub locations: Vec<String>,
    /// Whether the prompt's buttons should be disabled (e.g. while the
    /// reply is in flight, or after the user has already chosen).
    #[props(default = false)]
    pub disabled: bool,
    /// Fires when the user makes a choice.
    pub on_decision: EventHandler<PermissionDecision>,
}

#[component]
pub fn PermissionPrompt(props: PermissionPromptProps) -> Element {
    let id = props.id.clone();
    let kind = props.kind.clone();
    let title = props.title.clone();
    let locations = props.locations.clone();
    let disabled = props.disabled;
    let on_once = {
        let h = props.on_decision;
        move |_| h.call(PermissionDecision::AllowOnce)
    };
    let on_always = {
        let h = props.on_decision;
        move |_| h.call(PermissionDecision::AllowAlways)
    };
    let on_reject = {
        let h = props.on_decision;
        move |_| h.call(PermissionDecision::Reject)
    };

    rsx! {
        div {
            class: "operon-permission-prompt",
            "data-testid": "permission-prompt",
            "data-permission-id": id.clone(),
            "data-permission-kind": kind.clone(),
            div {
                class: "operon-permission-prompt-header",
                span { class: "operon-permission-prompt-kind", "{kind}" }
                span { class: "operon-permission-prompt-title", "{title}" }
            }
            if !locations.is_empty() {
                ul {
                    class: "operon-permission-prompt-locations",
                    for path in locations.iter() {
                        li { class: "operon-permission-prompt-location",
                            code { "{path}" }
                        }
                    }
                }
            }
            div {
                class: "operon-permission-prompt-actions",
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-once",
                    disabled: disabled,
                    onclick: on_once,
                    "Allow once"
                }
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-always",
                    disabled: disabled,
                    onclick: on_always,
                    "Always allow"
                }
                button {
                    class: "operon-permission-prompt-btn operon-permission-prompt-btn-reject",
                    disabled: disabled,
                    onclick: on_reject,
                    "Reject"
                }
            }
        }
    }
}

/// Translate the Dioxus-side `PermissionDecision` to the operon-core one
/// that `PermissionGate::reply` expects. Kept in this module so consumers
/// don't import the core type until the event is dispatched.
#[cfg(not(target_arch = "wasm32"))]
impl From<PermissionDecision> for operon_core::permission::PermissionDecision {
    fn from(d: PermissionDecision) -> Self {
        match d {
            PermissionDecision::AllowOnce => operon_core::permission::PermissionDecision::AllowOnce,
            PermissionDecision::AllowAlways => {
                operon_core::permission::PermissionDecision::AllowAlways
            }
            PermissionDecision::Reject => operon_core::permission::PermissionDecision::Reject,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_to_core_round_trip() {
        for (ui, expected) in [
            (
                PermissionDecision::AllowOnce,
                operon_core::permission::PermissionDecision::AllowOnce,
            ),
            (
                PermissionDecision::AllowAlways,
                operon_core::permission::PermissionDecision::AllowAlways,
            ),
            (
                PermissionDecision::Reject,
                operon_core::permission::PermissionDecision::Reject,
            ),
        ] {
            let core: operon_core::permission::PermissionDecision = ui.into();
            assert_eq!(core, expected);
        }
    }
}
