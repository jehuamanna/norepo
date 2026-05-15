//! Per-project tool-permissions section in Settings.
//!
//! Renders five checkboxes — one per [`ToolCategory`] — that gate the
//! permission-bridge handler's category auto-approve. Toggled values
//! land in `<active_repo>/.claude/settings.local.json` under the
//! `operonAutoApprove` key (see [`crate::shell::auto_approve`]).
//!
//! The policy is per-repo, not global, so this section needs an
//! active repository bound. When none is bound, the section renders
//! a guidance message and keeps the toggles disabled.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use dioxus::prelude::*;

use crate::shell::auto_approve::{self, AutoApprovePolicy, ToolOverride};
use crate::shell::companion_state::ActiveRepoPath;
use crate::shell::tool_category::ToolCategory;

/// Which field of `AutoApprovePolicy` an onchange handler should
/// toggle. Keeps the per-row click-handler closures small (one enum
/// variant captured by value) instead of capturing a unique callback.
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
pub fn ToolPermissionsSection() -> Element {
    let ActiveRepoPath(active_repo) = use_context();
    let repo_path: Option<PathBuf> = active_repo.read().clone();
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);

    let policy = {
        let _ = refresh_token.read();
        match repo_path.as_ref() {
            Some(p) => auto_approve::load(p),
            None => AutoApprovePolicy::default(),
        }
    };
    let disabled = repo_path.is_none();

    let row = |field: Field, help: &'static str| {
        let category = field.category();
        let current = field.current(&policy);
        rsx! {
            label {
                style: "display: flex; gap: 8px; align-items: center; padding: 4px 0; font-size: 0.9em;",
                "data-testid": format!("autoapprove-{}", category.settings_key()),
                input {
                    r#type: "checkbox",
                    checked: current,
                    disabled: disabled,
                    onchange: move |evt| {
                        let Some(repo) = active_repo.read().clone() else {
                            return;
                        };
                        let mut p = auto_approve::load(&repo);
                        field.apply(&mut p, evt.checked());
                        if let Err(e) = auto_approve::save(&repo, p) {
                            tracing::warn!(
                                target: "operon::permission",
                                "save AutoApprovePolicy for {}: {e}",
                                repo.display()
                            );
                        }
                        refresh_token.with_mut(|t| *t += 1);
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

    rsx! {
        div {
            style: "margin-top: 1rem;",
            "data-testid": "settings-tool-permissions",
            h3 {
                style: "margin: 0 0 0.5rem 0; font-size: 0.95em;",
                "Tool permissions (auto-approve)"
            }
            p {
                class: "operon-modal-help",
                style: "font-size: 0.8em; color: var(--operon-fg-muted, #666); margin: 0 0 0.5rem 0;",
                "Tool calls in the checked categories run without prompting. Stored per-project in .claude/settings.local.json under operonAutoApprove."
            }
            if disabled {
                p {
                    class: "operon-modal-help",
                    style: "font-style: italic; font-size: 0.85em;",
                    "Bind a project repository to configure tool permissions."
                }
            }
            {row(Field::ReadOnly, "Read, Glob, Grep, LS, NotebookRead")}
            {row(Field::FsWrite, "Write, Edit, NotebookEdit, MultiEdit")}
            {row(Field::Shell, "Bash")}
            {row(Field::Network, "WebFetch, WebSearch")}
            {row(Field::Other, "MCP server tools, unknown tools")}
            // Per-tool overrides — beat the per-category default.
            {
                let policy_for_overrides = policy.clone();
                rsx! {
                    OverrideEditor {
                        policy: policy_for_overrides,
                        disabled: disabled,
                        on_change: move |new_policy: AutoApprovePolicy| {
                            let Some(repo) = active_repo.read().clone() else {
                                return;
                            };
                            if let Err(e) = auto_approve::save(&repo, new_policy) {
                                tracing::warn!(
                                    target: "operon::permission",
                                    "save AutoApprovePolicy for {}: {e}",
                                    repo.display()
                                );
                            }
                            refresh_token.with_mut(|t| *t += 1);
                        },
                    }
                }
            }
            // Cascade opt-in: by default cascades force acceptEdits so
            // SDLC runs don't block on file-write prompts. Flipping
            // this on routes cascade tool calls through the same
            // per-category gate as interactive chats — useful for
            // dry-run-style cascades or when the user wants to review
            // each Edit before it lands.
            label {
                style: "display: flex; gap: 8px; align-items: center; padding: 8px 0 4px 0; font-size: 0.9em; border-top: 1px solid var(--operon-border, #ddd); margin-top: 8px;",
                "data-testid": "autoapprove-cascade-opt-in",
                input {
                    r#type: "checkbox",
                    checked: policy.cascade_uses_auto_approve_policy,
                    disabled: disabled,
                    onchange: move |evt| {
                        let Some(repo) = active_repo.read().clone() else {
                            return;
                        };
                        let mut p = auto_approve::load(&repo);
                        p.cascade_uses_auto_approve_policy = evt.checked();
                        if let Err(e) = auto_approve::save(&repo, p) {
                            tracing::warn!(
                                target: "operon::permission",
                                "save AutoApprovePolicy for {}: {e}",
                                repo.display()
                            );
                        }
                        refresh_token.with_mut(|t| *t += 1);
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
            // Phase 6 experimental: hijack claude's Bash through
            // Operon's own runner so the chat-side tool card gets
            // streaming stdout/stderr + a Cancel button (parity with
            // what the runtime backend already does).
            label {
                style: "display: flex; gap: 8px; align-items: center; padding: 4px 0; font-size: 0.9em;",
                "data-testid": "autoapprove-bash-via-operon",
                input {
                    r#type: "checkbox",
                    checked: policy.bash_via_operon,
                    disabled: disabled,
                    onchange: move |evt| {
                        let Some(repo) = active_repo.read().clone() else {
                            return;
                        };
                        let mut p = auto_approve::load(&repo);
                        p.bash_via_operon = evt.checked();
                        if let Err(e) = auto_approve::save(&repo, p) {
                            tracing::warn!(
                                target: "operon::permission",
                                "save AutoApprovePolicy for {}: {e}",
                                repo.display()
                            );
                        }
                        refresh_token.with_mut(|t| *t += 1);
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
}

#[derive(Props, Clone, PartialEq)]
struct OverrideEditorProps {
    policy: AutoApprovePolicy,
    disabled: bool,
    on_change: EventHandler<AutoApprovePolicy>,
}

#[component]
fn OverrideEditor(props: OverrideEditorProps) -> Element {
    let mut new_tool: Signal<String> = use_signal(String::new);
    let mut new_kind: Signal<String> = use_signal(|| "always_prompt".to_string());

    let policy_for_remove = props.policy.clone();
    let policy_for_add = props.policy.clone();
    let on_change_for_remove = props.on_change;
    let on_change_for_add = props.on_change;
    let on_remove = move |key: String| {
        let mut p = policy_for_remove.clone();
        p.tool_overrides.remove(&key);
        on_change_for_remove.call(p);
    };
    let on_add = move |_| {
        let tool = new_tool.read().trim().to_string();
        if tool.is_empty() {
            return;
        }
        let kind = match new_kind.read().as_str() {
            "always_allow" => ToolOverride::AlwaysAllow,
            _ => ToolOverride::AlwaysPrompt,
        };
        let mut p = policy_for_add.clone();
        p.tool_overrides.insert(tool, kind);
        on_change_for_add.call(p);
        new_tool.set(String::new());
    };

    let entries: Vec<(String, ToolOverride)> = props
        .policy
        .tool_overrides
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();

    rsx! {
        div {
            style: "margin-top: 0.75rem; padding-top: 0.5rem; border-top: 1px solid var(--operon-border, #ddd);",
            "data-testid": "settings-tool-overrides",
            h4 {
                style: "margin: 0 0 0.4rem 0; font-size: 0.9em;",
                "Per-tool overrides"
            }
            p {
                class: "operon-modal-help",
                style: "font-size: 0.8em; color: var(--operon-fg-muted, #666); margin: 0 0 0.5rem 0;",
                "Beat the category default for specific tools. e.g. \"Always prompt for Bash(git push *)\" or \"Always allow Edit\"."
            }
            if entries.is_empty() {
                p { class: "operon-modal-help",
                    style: "font-style: italic; font-size: 0.85em;",
                    "No overrides yet."
                }
            } else {
                ul {
                    style: "list-style: none; padding: 0; margin: 0 0 0.5rem 0;",
                    for (key, kind) in entries.iter() {
                        {
                            let key_for_label = key.clone();
                            let key_for_remove = key.clone();
                            let badge = match kind {
                                ToolOverride::AlwaysAllow => "Always allow",
                                ToolOverride::AlwaysPrompt => "Always prompt",
                            };
                            let mut on_remove_clone = on_remove.clone();
                            rsx! {
                                li {
                                    key: "{key_for_label}",
                                    style: "display: flex; gap: 8px; align-items: center; padding: 2px 0;",
                                    code { class: "md-inline-code", style: "flex: 1;",
                                        "{key_for_label}"
                                    }
                                    span { style: "font-size: 0.8em; color: var(--operon-fg-muted, #666);",
                                        "{badge}"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "operon-modal-button",
                                        style: "padding: 2px 8px; font-size: 0.8em;",
                                        disabled: props.disabled,
                                        onclick: move |_| on_remove_clone(key_for_remove.clone()),
                                        "Remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div {
                style: "display: flex; gap: 6px; align-items: center;",
                input {
                    r#type: "text",
                    "data-testid": "override-tool-input",
                    placeholder: "Tool name or Bash(npm install *)",
                    value: "{new_tool}",
                    disabled: props.disabled,
                    style: "flex: 1; padding: 4px 6px; font-size: 0.85em;",
                    oninput: move |evt| new_tool.set(evt.value()),
                }
                select {
                    "data-testid": "override-kind-select",
                    value: "{new_kind}",
                    disabled: props.disabled,
                    style: "padding: 4px 6px; font-size: 0.85em;",
                    onchange: move |evt| new_kind.set(evt.value()),
                    option { value: "always_prompt", "Always prompt" }
                    option { value: "always_allow", "Always allow" }
                }
                button {
                    r#type: "button",
                    class: "operon-modal-button",
                    "data-testid": "override-add-btn",
                    disabled: props.disabled,
                    style: "padding: 4px 10px; font-size: 0.85em;",
                    onclick: on_add,
                    "Add"
                }
            }
        }
    }
}
