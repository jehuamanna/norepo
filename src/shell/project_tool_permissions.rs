//! Per-project Tool Permissions modal.
//!
//! Opened from the explorer project row (gear icon + "Tool permissions…"
//! context-menu entry). Replaces the old global-Settings section that
//! couldn't function without an active repo binding.
//!
//! Each project's policy is per-repo: writes land in
//! `<repo>/.claude/settings.local.json` under `operonAutoApprove`
//! (see [`crate::shell::auto_approve`]). When the target project has no
//! `repo_path` bound yet, the modal renders an inline "Bind repository…"
//! action that opens the OS folder picker and persists the choice via
//! [`operon_store::repos::LocalProjectRepository::set_repo_path`] — the
//! next render of the modal then exposes the toggles.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use dioxus::prelude::*;
use uuid::Uuid;

use crate::local_mode::desktop::LocalProjectRepo;
use crate::shell::auto_approve::{self, AutoApprovePolicy, ToolOverride};
use crate::shell::tool_category::ToolCategory;

/// App-scope handle. `Some(project_id)` opens the modal scoped to that
/// project; `None` keeps it closed. Provided in `App`; written by the
/// project row's gear icon and context-menu entry; cleared by the
/// modal's own close paths (Esc / scrim / Close button).
#[derive(Clone, Copy)]
pub struct ProjectToolPermissionsTarget(pub Signal<Option<Uuid>>);

/// Which field of `AutoApprovePolicy` an onchange handler should
/// toggle. Same shape as the original `tool_permissions::Field`.
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
pub fn ProjectToolPermissionsPanel() -> Element {
    let ProjectToolPermissionsTarget(mut target) = use_context();
    let pid_opt: Option<Uuid> = *target.read();
    let Some(pid) = pid_opt else {
        return rsx! {};
    };

    let LocalProjectRepo(project_repo) = use_context();

    // Bumped on every persisted change so the body re-reads the project
    // row (for repo_path) and the policy file (for toggle state).
    let mut refresh_token: Signal<u64> = use_signal(|| 0u64);

    let project = {
        let _ = refresh_token.read();
        project_repo
            .list()
            .ok()
            .and_then(|projects| projects.into_iter().find(|p| p.id == pid))
    };
    let Some(project) = project else {
        // Project was deleted out from under the modal — close it.
        target.set(None);
        return rsx! {};
    };

    let project_name = project.name.clone();
    let repo_path: Option<PathBuf> = project.repo_path.clone();
    let policy = match repo_path.as_ref() {
        Some(p) => auto_approve::load(p),
        None => AutoApprovePolicy::default(),
    };

    let close = move |_| target.set(None);

    rsx! {
        div {
            class: "operon-modal-scrim",
            "data-testid": "project-tool-permissions-panel",
            "data-project-id": "{pid}",
            onclick: close,
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    evt.prevent_default();
                    target.set(None);
                }
            },
            tabindex: "0",
            div {
                class: "operon-modal-card",
                style: "max-width: 640px; max-height: 80vh; display: flex; flex-direction: column;",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "operon-modal-title",
                    "Tool permissions — {project_name}"
                }
                p {
                    class: "operon-modal-message",
                    "Tool calls in the checked categories run without prompting. Stored in the project repository's .claude/settings.local.json under operonAutoApprove."
                }
                div {
                    style: "overflow-y: auto; flex: 1; margin-top: 12px;",
                    if let Some(repo) = repo_path.clone() {
                        Body {
                            project_id: pid,
                            repo_path: repo,
                            policy: policy.clone(),
                            on_changed: move |_: ()| {
                                refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                            },
                        }
                    } else {
                        BindRepoPrompt {
                            project_id: pid,
                            on_bound: move |_: ()| {
                                refresh_token.with_mut(|t| *t = t.wrapping_add(1));
                            },
                        }
                    }
                }
                div {
                    class: "operon-modal-actions",
                    button {
                        r#type: "button",
                        class: "operon-modal-button operon-modal-button-primary",
                        "data-testid": "project-tool-permissions-close",
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
    project_id: Uuid,
    repo_path: PathBuf,
    policy: AutoApprovePolicy,
    on_changed: EventHandler<()>,
}

#[component]
fn Body(props: BodyProps) -> Element {
    let policy = props.policy.clone();
    let repo_for_rows = props.repo_path.clone();
    let on_changed_for_rows = props.on_changed;

    let row = move |field: Field, help: &'static str| {
        let category = field.category();
        let current = field.current(&policy);
        let repo = repo_for_rows.clone();
        let on_changed = on_changed_for_rows;
        rsx! {
            label {
                style: "display: flex; gap: 8px; align-items: center; padding: 4px 0; font-size: 0.9em;",
                "data-testid": format!("autoapprove-{}", category.settings_key()),
                input {
                    r#type: "checkbox",
                    checked: current,
                    onchange: move |evt| {
                        let mut p = auto_approve::load(&repo);
                        field.apply(&mut p, evt.checked());
                        if let Err(e) = auto_approve::save(&repo, p) {
                            tracing::warn!(
                                target: "operon::permission",
                                "save AutoApprovePolicy for {}: {e}",
                                repo.display()
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

    let cascade_repo = props.repo_path.clone();
    let bash_repo = props.repo_path.clone();
    let cascade_on_changed = props.on_changed;
    let bash_on_changed = props.on_changed;
    let cascade_checked = props.policy.cascade_uses_auto_approve_policy;
    let bash_checked = props.policy.bash_via_operon;

    let override_policy = props.policy.clone();
    let override_repo = props.repo_path.clone();
    let override_on_changed = props.on_changed;

    rsx! {
        {row(Field::ReadOnly, "Read, Glob, Grep, LS, NotebookRead")}
        {row(Field::FsWrite, "Write, Edit, NotebookEdit, MultiEdit")}
        {row(Field::Shell, "Bash")}
        {row(Field::Network, "WebFetch, WebSearch")}
        {row(Field::Other, "MCP server tools, unknown tools")}
        OverrideEditor {
            policy: override_policy,
            on_change: move |new_policy: AutoApprovePolicy| {
                if let Err(e) = auto_approve::save(&override_repo, new_policy) {
                    tracing::warn!(
                        target: "operon::permission",
                        "save AutoApprovePolicy for {}: {e}",
                        override_repo.display()
                    );
                }
                override_on_changed.call(());
            },
        }
        // Cascade opt-in: by default cascades force acceptEdits so SDLC
        // runs don't block on file-write prompts. Flipping this on routes
        // cascade tool calls through the same per-category gate as
        // interactive chats — useful for dry-run-style cascades or when
        // the user wants to review each Edit before it lands.
        label {
            style: "display: flex; gap: 8px; align-items: center; padding: 8px 0 4px 0; font-size: 0.9em; border-top: 1px solid var(--operon-border, #ddd); margin-top: 8px;",
            "data-testid": "autoapprove-cascade-opt-in",
            input {
                r#type: "checkbox",
                checked: cascade_checked,
                onchange: move |evt| {
                    let mut p = auto_approve::load(&cascade_repo);
                    p.cascade_uses_auto_approve_policy = evt.checked();
                    if let Err(e) = auto_approve::save(&cascade_repo, p) {
                        tracing::warn!(
                            target: "operon::permission",
                            "save AutoApprovePolicy for {}: {e}",
                            cascade_repo.display()
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
        // Phase 6 experimental: hijack claude's Bash through Operon's own
        // runner so the chat-side tool card gets streaming stdout/stderr
        // + a Cancel button (parity with the runtime backend).
        label {
            style: "display: flex; gap: 8px; align-items: center; padding: 4px 0; font-size: 0.9em;",
            "data-testid": "autoapprove-bash-via-operon",
            input {
                r#type: "checkbox",
                checked: bash_checked,
                onchange: move |evt| {
                    let mut p = auto_approve::load(&bash_repo);
                    p.bash_via_operon = evt.checked();
                    if let Err(e) = auto_approve::save(&bash_repo, p) {
                        tracing::warn!(
                            target: "operon::permission",
                            "save AutoApprovePolicy for {}: {e}",
                            bash_repo.display()
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

#[derive(Props, Clone, PartialEq)]
struct BindRepoPromptProps {
    project_id: Uuid,
    on_bound: EventHandler<()>,
}

#[component]
fn BindRepoPrompt(props: BindRepoPromptProps) -> Element {
    let LocalProjectRepo(project_repo) = use_context();
    let pid = props.project_id;
    let on_bound = props.on_bound;

    let pick_repo = move |_| {
        let project_repo = project_repo.clone();
        let on_bound = on_bound;
        spawn(async move {
            let folder = rfd::AsyncFileDialog::new()
                .set_title("Select repository folder for this project")
                .pick_folder()
                .await;
            if let Some(handle) = folder {
                let path = handle.path().to_path_buf();
                if let Err(e) = project_repo.set_repo_path(pid, Some(&path)) {
                    tracing::warn!(
                        target: "operon::project",
                        "set_repo_path({pid}) failed: {e}"
                    );
                    return;
                }
                on_bound.call(());
            }
        });
    };

    rsx! {
        div {
            "data-testid": "project-tool-permissions-bind-prompt",
            style: "padding: 8px 0;",
            p {
                class: "operon-modal-message",
                "This project has no repository bound. Tool permissions are stored in the repo's ",
                code { class: "md-inline-code", ".claude/settings.local.json" }
                " — bind a repository to enable the toggles."
            }
            div {
                style: "margin-top: 12px;",
                button {
                    r#type: "button",
                    class: "operon-modal-button operon-modal-button-primary",
                    "data-testid": "project-tool-permissions-bind-repo",
                    onclick: pick_repo,
                    "Bind repository\u{2026}"
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub(crate) struct OverrideEditorProps {
    pub policy: AutoApprovePolicy,
    pub on_change: EventHandler<AutoApprovePolicy>,
}

#[component]
pub(crate) fn OverrideEditor(props: OverrideEditorProps) -> Element {
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
                            let on_remove_clone = on_remove.clone();
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
                    style: "flex: 1; padding: 4px 6px; font-size: 0.85em;",
                    oninput: move |evt| new_tool.set(evt.value()),
                }
                select {
                    "data-testid": "override-kind-select",
                    value: "{new_kind}",
                    style: "padding: 4px 6px; font-size: 0.85em;",
                    onchange: move |evt| new_kind.set(evt.value()),
                    option { value: "always_prompt", "Always prompt" }
                    option { value: "always_allow", "Always allow" }
                }
                button {
                    r#type: "button",
                    class: "operon-modal-button",
                    "data-testid": "override-add-btn",
                    onclick: on_add,
                    "Add"
                }
            }
        }
    }
}
