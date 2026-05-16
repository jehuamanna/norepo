//! Global (user-scope) Chat Permissions modal.
//!
//! Opened from the global Settings dialog. Mirrors the per-project Tool
//! Permissions modal ([`crate::shell::project_tool_permissions`]) but
//! writes to the user-scope `~/.claude/settings.json` (under the same
//! `operonAutoApprove` key) instead of a per-repo
//! `<repo>/.claude/settings.local.json`.
//!
//! Resolution order at enforcement time (see
//! [`crate::shell::auto_approve::load_effective`]):
//!
//! 1. Project file (`<repo>/.claude/settings.local.json`) if it has an
//!    `operonAutoApprove` key.
//! 2. Else the global file edited by this modal.
//! 3. Else [`crate::shell::auto_approve::AutoApprovePolicy::default`].
//!
//! Per-project tool permissions therefore *override* whatever is set
//! here; the global tier is the fallback for projects that have never
//! been configured.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::shell::auto_approve::{self, AutoApprovePolicy};
use crate::shell::project_tool_permissions::OverrideEditor;
use crate::shell::tool_category::ToolCategory;

/// App-scope visibility signal for the panel. Provided in `App`,
/// flipped by the Settings dialog button. The panel owns the close
/// write (Esc / scrim click / Close button).
#[derive(Clone, Copy)]
pub struct GlobalChatPermissionsOpen(pub Signal<bool>);

/// Which field of `AutoApprovePolicy` an onchange handler should
/// toggle. Same shape as the per-project modal's `Field` — duplicated
/// here to keep both modules' private state private.
#[derive(Clone, Copy)]
enum Field {
    ReadOnly,
    FsWrite,
    Shell,
    Network,
    Other,
}

impl Field {
    fn apply(self, p: &mut AutoApprovePolicy, value: bool) {
        match self {
            Field::ReadOnly => p.read_only = value,
            Field::FsWrite => p.fs_write = value,
            Field::Shell => p.shell = value,
            Field::Network => p.network = value,
            Field::Other => p.other = value,
        }
    }

    fn current(self, p: &AutoApprovePolicy) -> bool {
        match self {
            Field::ReadOnly => p.read_only,
            Field::FsWrite => p.fs_write,
            Field::Shell => p.shell,
            Field::Network => p.network,
            Field::Other => p.other,
        }
    }

    fn category(self) -> ToolCategory {
        match self {
            Field::ReadOnly => ToolCategory::ReadOnly,
            Field::FsWrite => ToolCategory::FsWrite,
            Field::Shell => ToolCategory::Shell,
            Field::Network => ToolCategory::Network,
            Field::Other => ToolCategory::Other,
        }
    }
}

#[component]
pub fn GlobalChatPermissionsPanel() -> Element {
    let GlobalChatPermissionsOpen(mut open) = use_context();
    if !*open.read() {
        return rsx! {};
    }

    // Bumped on every persisted change so the body re-reads the global
    // policy file and re-renders with the fresh toggle state.
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);
    let policy = {
        let _ = refresh_token.read();
        auto_approve::load_global()
    };
    let global_path_label = auto_approve::global_settings_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.claude/settings.json".to_string());

    let close = move |_| open.set(false);

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "global-chat-permissions-panel",
            onclick: close,
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    open.set(false);
                }
            },
            tabindex: "0",
            div {
                class: "operon-modal-card",
                style: "max-width: 640px; max-height: 80vh; display: flex; flex-direction: column;",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title", "Chat permissions \u{2014} Global" }
                p {
                    class: "operon-modal-message",
                    "Tool calls in the checked categories run without prompting in every chat. \
                     Stored in "
                    code { class: "md-inline-code", "{global_path_label}" }
                    ". Per-project tool permissions, when set, override these globals."
                }
                div {
                    style: "overflow-y: auto; flex: 1; margin-top: 12px;",
                    Body {
                        policy: policy.clone(),
                        on_changed: move |_: ()| {
                            refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                        },
                    }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "global-chat-permissions-close",
                        onclick: close,
                        "Close"
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct BodyProps {
    policy: AutoApprovePolicy,
    on_changed: EventHandler<()>,
}

#[component]
fn Body(props: BodyProps) -> Element {
    let policy = props.policy.clone();
    let on_changed_for_rows = props.on_changed;

    let row = move |field: Field, help: &'static str| {
        let category = field.category();
        let current = field.current(&policy);
        let on_changed = on_changed_for_rows;
        rsx! {
            label {
                style: "display: flex; gap: 8px; align-items: center; padding: 4px 0; font-size: 0.9em;",
                "data-testid": format!("global-autoapprove-{}", category.settings_key()),
                input {
                    r#type: "checkbox",
                    checked: current,
                    onchange: move |evt| {
                        let mut p = auto_approve::load_global();
                        field.apply(&mut p, evt.checked());
                        if let Err(e) = auto_approve::save_global(p) {
                            tracing::warn!(
                                target: "operon::permission",
                                "save global AutoApprovePolicy: {e}"
                            );
                        }
                        on_changed.call(());
                    },
                }
                span { style: "min-width: 140px; font-weight: 500;",
                    "{category.label()}"
                }
                span { class: "operon-modal-help",
                    style: "font-size: 0.8em; color: var(--operon-fg-muted, #666);",
                    "{help}"
                }
            }
        }
    };

    let cascade_on_changed = props.on_changed;
    let bash_on_changed = props.on_changed;
    let cascade_checked = props.policy.cascade_uses_auto_approve_policy;
    let bash_checked = props.policy.bash_via_operon;
    let override_on_changed = props.on_changed;
    let override_policy = props.policy.clone();

    rsx! {
        {row(Field::ReadOnly, "Read, Glob, Grep, LS, NotebookRead")}
        {row(Field::FsWrite, "Write, Edit, NotebookEdit, MultiEdit")}
        {row(Field::Shell, "Bash")}
        {row(Field::Network, "WebFetch, WebSearch")}
        {row(Field::Other, "MCP server tools, unknown tools")}
        OverrideEditor {
            policy: override_policy,
            on_change: move |new_policy: AutoApprovePolicy| {
                if let Err(e) = auto_approve::save_global(new_policy) {
                    tracing::warn!(
                        target: "operon::permission",
                        "save global AutoApprovePolicy: {e}"
                    );
                }
                override_on_changed.call(());
            },
        }
        label {
            style: "display: flex; gap: 8px; align-items: center; padding: 8px 0 4px 0; font-size: 0.9em; border-top: 1px solid var(--operon-border, #ddd); margin-top: 8px;",
            "data-testid": "global-autoapprove-cascade-opt-in",
            input {
                r#type: "checkbox",
                checked: cascade_checked,
                onchange: move |evt| {
                    let mut p = auto_approve::load_global();
                    p.cascade_uses_auto_approve_policy = evt.checked();
                    if let Err(e) = auto_approve::save_global(p) {
                        tracing::warn!(
                            target: "operon::permission",
                            "save global AutoApprovePolicy: {e}"
                        );
                    }
                    cascade_on_changed.call(());
                },
            }
            span { style: "min-width: 140px; font-weight: 500;",
                "Cascades use policy"
            }
            span { class: "operon-modal-help",
                style: "font-size: 0.8em; color: var(--operon-fg-muted, #666);",
                "Off (default): cascades auto-approve Edit/Write. On: cascades honor the toggles above just like interactive chats."
            }
        }
        label {
            style: "display: flex; gap: 8px; align-items: center; padding: 4px 0; font-size: 0.9em;",
            "data-testid": "global-autoapprove-bash-via-operon",
            input {
                r#type: "checkbox",
                checked: bash_checked,
                onchange: move |evt| {
                    let mut p = auto_approve::load_global();
                    p.bash_via_operon = evt.checked();
                    if let Err(e) = auto_approve::save_global(p) {
                        tracing::warn!(
                            target: "operon::permission",
                            "save global AutoApprovePolicy: {e}"
                        );
                    }
                    bash_on_changed.call(());
                },
            }
            span { style: "min-width: 140px; font-weight: 500;",
                "Run Bash via Operon"
            }
            span { class: "operon-modal-help",
                style: "font-size: 0.8em; color: var(--operon-fg-muted, #666);",
                "Experimental. Streams Bash output live in the chat with per-call Cancel. Restart chat session to take effect. Pre-existing Bash(...) allow rules need to be re-keyed to mcp__operon__operon_bash(...)."
            }
        }
    }
}
