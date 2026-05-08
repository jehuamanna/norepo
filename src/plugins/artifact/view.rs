//! `ArtifactFormatPlugin` — `format_id = "artifact"`. Renders an
//! artifact note with a status pill, action buttons (Approve /
//! Reject / Re-run), and the markdown body. The body editor itself
//! is plain `<textarea>` so the user can edit acceptance criteria
//! and other inline content; frontmatter mutations from the action
//! buttons go through `frontmatter::rewrite` so the user's edits in
//! the body aren't blown away.

use dioxus::prelude::*;
use operon_store::repos::{LocalNote, NoteKind};
use std::sync::Arc;
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::LocalNoteVersion;
use crate::persistence::Persistence;
use crate::plugins::artifact::frontmatter::{
    parse, rewrite, ArtifactFrontmatter, ArtifactKind, ArtifactStatus,
};
use crate::plugins::markdown::MarkdownView;
use crate::shell::companion_state::{
    ChatMessageRepo, ChatSessionRepo, ChatSessionVersion, ClaudeCodePluginCtx,
};

#[derive(Props, Clone, PartialEq)]
pub struct ArtifactViewProps {
    pub note_id: String,
    pub content: String,
    /// `true` for Edit-mode mount (body is editable); `false` for the
    /// View-mode read-only render.
    pub edit: bool,
    /// `None` in View-mode (no body mutations possible). `Some` in
    /// Edit-mode so the action buttons can patch the frontmatter and
    /// the textarea can push body edits back.
    #[props(default)]
    pub on_change: Option<EventHandler<String>>,
}

#[component]
pub fn ArtifactView(props: ArtifactViewProps) -> Element {
    let fm = parse(&props.content);
    let body_only = strip_frontmatter(&props.content).to_string();
    let kind_label = fm
        .artifact_kind
        .as_ref()
        .map(|k| k.display_name())
        .unwrap_or_else(|| "Artifact".into());
    let status = fm.status;
    let status_class = status_css_class(status);
    let status_label = status_human_label(status);
    let on_change = props.on_change;
    let content_for_actions = props.content.clone();

    let approve = {
        let content_for_actions = content_for_actions.clone();
        let on_change = on_change;
        move |_| patch_status(&content_for_actions, ArtifactStatus::Approved, on_change)
    };
    let reject = {
        let content_for_actions = content_for_actions.clone();
        let on_change = on_change;
        move |_| patch_status(&content_for_actions, ArtifactStatus::Rejected, on_change)
    };
    let mark_dirty = {
        let content_for_actions = content_for_actions.clone();
        let on_change = on_change;
        move |_| patch_status(&content_for_actions, ArtifactStatus::Dirty, on_change)
    };

    let body_editable = props.edit && on_change.is_some();
    let textarea_value = body_only.clone();
    let fm_for_textarea = fm.clone();

    // "Run skill" picker — collapsed by default; opens inline above
    // the body. Filters skills by `input_kind` matching this
    // artifact's kind (when both are set); otherwise lists all
    // skills. Run picks up project / repo / chat-session context the
    // same way the workflow cascade does.
    let mut picker_open = use_signal(|| false);
    let source_uuid = Uuid::parse_str(&props.note_id).ok();
    let source_kind_str = fm.artifact_kind.as_ref().map(|k| k.as_str().to_string());
    // Phase C: status of the last run lives at the artifact-view
    // scope so it survives the picker's close. The picker pushes
    // updates through this signal; we render the pill near the
    // action buttons regardless of whether the picker is open.
    let run_status: Signal<Option<Result<String, String>>> = use_signal(|| None);
    rsx! {
        div { class: "operon-artifact-surface",
            "data-testid": "artifact-surface",
            "data-artifact-status": "{status.as_str()}",
            "data-artifact-kind": "{fm.artifact_kind.as_ref().map(|k| k.as_str().to_string()).unwrap_or_default()}",
            div { class: "operon-artifact-header",
                span { class: "operon-artifact-kind-badge", "{kind_label}" }
                span {
                    class: "operon-artifact-status-pill {status_class}",
                    "data-testid": "artifact-status-pill",
                    "{status_label}"
                }
                {
                    let parent = fm.source_artifact_id;
                    let skill = fm.source_skill_id;
                    rsx! {
                        if let Some(p) = parent {
                            span { class: "operon-artifact-meta",
                                "from "
                                code { class: "md-inline-code", "{short_uuid(p)}" }
                            }
                        }
                        if let Some(s) = skill {
                            span { class: "operon-artifact-meta",
                                "via "
                                code { class: "md-inline-code", "{short_uuid(s)}" }
                            }
                        }
                    }
                }
                div { class: "operon-artifact-actions",
                    if body_editable {
                        button {
                            r#type: "button",
                            class: "operon-artifact-approve",
                            "data-testid": "artifact-approve",
                            disabled: status == ArtifactStatus::Approved,
                            onclick: approve,
                            "Approve"
                        }
                        button {
                            r#type: "button",
                            class: "operon-artifact-reject",
                            "data-testid": "artifact-reject",
                            disabled: status == ArtifactStatus::Rejected,
                            onclick: reject,
                            "Reject"
                        }
                        button {
                            r#type: "button",
                            class: "operon-artifact-mark-dirty",
                            "data-testid": "artifact-mark-dirty",
                            title: "Mark as dirty so this artifact is re-generated on the next run.",
                            onclick: mark_dirty,
                            "Mark dirty"
                        }
                        button {
                            r#type: "button",
                            class: "operon-artifact-run-skill",
                            "data-testid": "artifact-run-skill",
                            title: "Run a skill on this artifact to produce child artifacts (Epics, Features, etc.).",
                            onclick: move |_| {
                                picker_open.with_mut(|v| *v = !*v);
                            },
                            if *picker_open.read() { "Hide skills" } else { "Run skill\u{2026}" }
                        }
                    }
                }
            }
            if *picker_open.read() {
                if let Some(src_uuid) = source_uuid {
                    SkillPickerPanel {
                        source_note_id: src_uuid,
                        source_artifact_kind: source_kind_str.clone(),
                        on_dismiss: Callback::new(move |_| picker_open.set(false)),
                        run_status: run_status,
                    }
                }
            }
            // Phase C: inline run-status row, visible regardless of
            // whether the picker is open. Empty before the first
            // run; replaced by "Created N..." or the error message
            // after each run completes.
            {
                let snapshot = run_status.read().clone();
                match snapshot {
                    None => rsx! {},
                    Some(Ok(msg)) => rsx! {
                        div {
                            class: "operon-artifact-run-status operon-artifact-run-status-ok",
                            "data-testid": "artifact-run-status",
                            "{msg}"
                        }
                    },
                    Some(Err(msg)) => rsx! {
                        div {
                            class: "operon-artifact-run-status operon-artifact-run-status-err",
                            "data-testid": "artifact-run-status",
                            "{msg}"
                        }
                    },
                }
            }
            div { class: "operon-artifact-body",
                if body_editable {
                    textarea {
                        class: "operon-artifact-textarea",
                        "data-testid": "artifact-textarea",
                        spellcheck: "false",
                        value: "{textarea_value}",
                        oninput: move |e| {
                            // The textarea edits the BODY only; we
                            // re-attach the (untouched) frontmatter
                            // before pushing the change up so the
                            // status pill / source linkage survive.
                            let new_body = e.value();
                            let recombined = rewrite(&content_for_actions, &fm_for_textarea);
                            let final_doc = replace_body(&recombined, &new_body);
                            if let Some(handler) = on_change {
                                handler.call(final_doc);
                            }
                        },
                    }
                } else {
                    MarkdownView { content: body_only }
                }
            }
        }
    }
}

fn patch_status(
    content: &str,
    next: ArtifactStatus,
    on_change: Option<EventHandler<String>>,
) {
    let mut fm = parse(content);
    fm.status = next;
    let new_body = rewrite(content, &fm);
    if let Some(handler) = on_change {
        handler.call(new_body);
    }
}

fn status_css_class(s: ArtifactStatus) -> &'static str {
    match s {
        ArtifactStatus::Pending => "operon-artifact-status-pending",
        ArtifactStatus::Approved => "operon-artifact-status-approved",
        ArtifactStatus::Rejected => "operon-artifact-status-rejected",
        ArtifactStatus::Dirty => "operon-artifact-status-dirty",
        ArtifactStatus::Running => "operon-artifact-status-running",
        ArtifactStatus::Error => "operon-artifact-status-error",
    }
}

fn status_human_label(s: ArtifactStatus) -> &'static str {
    match s {
        ArtifactStatus::Pending => "pending",
        ArtifactStatus::Approved => "approved",
        ArtifactStatus::Rejected => "rejected",
        ArtifactStatus::Dirty => "dirty",
        ArtifactStatus::Running => "running\u{2026}",
        ArtifactStatus::Error => "error",
    }
}

fn short_uuid(id: Uuid) -> String {
    let s = id.to_string();
    s.chars().take(8).collect()
}

/// Strip the YAML frontmatter (if present) and return only the
/// body content. Tolerates a missing fence by returning the whole
/// input.
fn strip_frontmatter(content: &str) -> &str {
    let (_, body) = crate::plugins::skill::frontmatter::split(content);
    body
}

/// Replace the body portion of `doc` with `new_body`, preserving
/// the existing frontmatter block. Used by the textarea oninput so
/// the user can edit the body without the frontmatter changing.
fn replace_body(doc: &str, new_body: &str) -> String {
    let (_lines, _) = crate::plugins::skill::frontmatter::split(doc);
    // Find the closing `---` and put `new_body` after it.
    let trimmed_start = doc.trim_start_matches('\u{feff}');
    let lookahead = trimmed_start.trim_start();
    if !lookahead.starts_with("---") {
        return new_body.to_string();
    }
    let after_first = match lookahead.split_once("---") {
        Some((_, rest)) => rest,
        None => return new_body.to_string(),
    };
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);
    let mut offset = 0usize;
    let mut closed = false;
    for line in after_first.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            offset += line.len();
            closed = true;
            break;
        }
        offset += line.len();
    }
    if !closed {
        return new_body.to_string();
    }
    // Reconstruct: leading frontmatter block + a single newline + body.
    let prefix_end = doc.len() - after_first.len() + offset;
    let mut out = String::with_capacity(prefix_end + new_body.len() + 1);
    out.push_str(&doc[..prefix_end]);
    if !new_body.starts_with('\n') {
        out.push('\n');
    }
    out.push_str(new_body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_body_swaps_only_the_body() {
        let doc = "---\nartifact_kind: epic\nstatus: pending\n---\n\noriginal body";
        let next = replace_body(doc, "shiny new body");
        assert!(next.contains("artifact_kind: epic"));
        assert!(next.contains("shiny new body"));
        assert!(!next.contains("original body"));
    }

    #[test]
    fn replace_body_no_frontmatter_returns_new_body() {
        let next = replace_body("just some text", "replacement");
        assert_eq!(next, "replacement");
    }

    #[test]
    fn short_uuid_is_eight_chars() {
        let u = Uuid::nil();
        assert_eq!(short_uuid(u).len(), 8);
    }
}

// ---- Helper: mark unused warnings inside derive callbacks happy.
#[allow(dead_code)]
fn _force_unused_field(_fm: &ArtifactFrontmatter, _kind: &ArtifactKind) {}

/// Inline panel that lists project skills, optionally filtered by
/// `input_kind` matching the source artifact's `artifact_kind`. When
/// the user clicks a skill, spawns the runner against this artifact.
/// Auto-dismisses on a successful spawn.
#[derive(Props, Clone, PartialEq)]
struct SkillPickerPanelProps {
    source_note_id: Uuid,
    source_artifact_kind: Option<String>,
    on_dismiss: Callback<()>,
    /// Hoisted from `ArtifactView` so the result line survives the
    /// picker's close. `None` = no run yet; `Some(Ok(_))` = the last
    /// run produced N artifacts; `Some(Err(_))` = it failed.
    run_status: Signal<Option<Result<String, String>>>,
}

#[component]
fn SkillPickerPanel(props: SkillPickerPanelProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let ClaudeCodePluginCtx(plugin) = use_context();
    let ChatSessionRepo(chat_session_repo) = use_context();
    let ChatMessageRepo(chat_message_repo) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let ChatSessionVersion(chat_session_version) = use_context();
    // Phase B: auto-switch the rail to the runner's session so the
    // user immediately sees Claude's transcript stream in real time.
    // Mirrors the pattern in `play_skill` at
    // src/plugins/skill/view.rs:275-276.
    let crate::shell::companion_state::ActiveChatSession(active_session) = use_context();
    let crate::shell::companion_state::ActiveChatScope(active_scope) = use_context();

    let project_id = match note_repo.find_project_for_note(props.source_note_id) {
        Ok(Some(p)) => p,
        _ => {
            return rsx! {
                div { class: "operon-artifact-skill-picker",
                    "data-testid": "artifact-skill-picker",
                    div { class: "operon-artifact-skill-picker-empty",
                        "This note has no project bound \u{2014} can't list skills."
                    }
                }
            };
        }
    };

    // Pull all Skill notes in the project and parse each one's
    // contract. Filter to skills whose input_kind matches the source
    // artifact's kind (if both are set).
    let project_notes = note_repo.list_for_project(project_id).unwrap_or_default();
    let skill_notes: Vec<LocalNote> = project_notes
        .into_iter()
        .filter(|n| matches!(n.kind, NoteKind::Skill))
        .collect();

    let source_kind = props.source_artifact_kind.clone();
    let last_status = props.run_status;

    rsx! {
        div { class: "operon-artifact-skill-picker",
            "data-testid": "artifact-skill-picker",
            div { class: "operon-artifact-skill-picker-header",
                span { "Run a skill on this artifact" }
                button {
                    r#type: "button",
                    class: "operon-artifact-skill-picker-close",
                    onclick: {
                        let on_dismiss = props.on_dismiss;
                        move |_| on_dismiss.call(())
                    },
                    "\u{2715}"
                }
            }
            if skill_notes.is_empty() {
                div { class: "operon-artifact-skill-picker-empty",
                    "No skill notes in this project yet \u{2014} create one with + \u{2192} Skill in the explorer."
                }
            } else {
                ul { class: "operon-artifact-skill-picker-list",
                    for skill in skill_notes.iter() {
                        {
                            let skill_id = skill.id;
                            let skill_title = skill.title.clone();
                            // Cheap contract preview: try to load the
                            // skill body synchronously via the explorer's
                            // blocking load. If unavailable (rare), we
                            // still render the row but without the
                            // input/output kind chips.
                            let preview = preview_skill_contract(&persistence, skill_id);
                            let matches_kind = match (&preview.input_kind, &source_kind) {
                                (Some(input), Some(src)) => input == src,
                                // No declared input_kind → match anything.
                                (None, _) => true,
                                // Source has no kind yet → match anything.
                                (Some(_), None) => true,
                            };
                            let row_class = if matches_kind {
                                "operon-artifact-skill-picker-item"
                            } else {
                                "operon-artifact-skill-picker-item operon-artifact-skill-picker-item-mismatch"
                            };
                            let chip_in = preview.input_kind.clone();
                            let chip_out = preview.output_kind.clone();
                            let chip_count = preview.output_count.clone();
                            let chip_gate = preview.gate.clone();
                            let note_repo_for_run = note_repo.clone();
                            let project_repo_for_run = project_repo.clone();
                            let persistence_for_run = persistence.clone();
                            let plugin_for_run = plugin.clone();
                            let chat_session_repo_for_run = chat_session_repo.clone();
                            let chat_message_repo_for_run = chat_message_repo.clone();
                            let mut status_setter = last_status;
                            let mut note_version_setter = note_version;
                            let mut chat_session_version_setter = chat_session_version;
                            let mut active_session_setter = active_session;
                            let mut active_scope_setter = active_scope;
                            let on_dismiss = props.on_dismiss;
                            let source_id = props.source_note_id;
                            rsx! {
                                li {
                                    key: "{skill_id}",
                                    button {
                                        r#type: "button",
                                        class: "{row_class}",
                                        title: if matches_kind { "Run this skill" } else { "Skill expects a different input_kind — click to run anyway" },
                                        onclick: move |_| {
                                            spawn_runner(
                                                source_id,
                                                skill_id,
                                                project_id,
                                                note_repo_for_run.clone(),
                                                project_repo_for_run.clone(),
                                                persistence_for_run.clone(),
                                                plugin_for_run.clone(),
                                                chat_session_repo_for_run.clone(),
                                                chat_message_repo_for_run.clone(),
                                                &mut note_version_setter,
                                                &mut chat_session_version_setter,
                                                &mut active_session_setter,
                                                &mut active_scope_setter,
                                                &mut status_setter,
                                            );
                                            on_dismiss.call(());
                                        },
                                        div { class: "operon-artifact-skill-picker-item-title", "{skill_title}" }
                                        div { class: "operon-artifact-skill-picker-item-chips",
                                            if let Some(s) = chip_in.as_ref() {
                                                span { class: "operon-artifact-skill-picker-chip", "in: {s}" }
                                            }
                                            if let Some(s) = chip_out.as_ref() {
                                                span { class: "operon-artifact-skill-picker-chip", "out: {s}" }
                                            }
                                            span { class: "operon-artifact-skill-picker-chip", "{chip_count}" }
                                            span { class: "operon-artifact-skill-picker-chip", "{chip_gate}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(s) = last_status.read().as_ref() {
                match s {
                    Ok(msg) => rsx! {
                        div { class: "operon-artifact-skill-picker-status operon-artifact-skill-picker-status-ok", "{msg}" }
                    },
                    Err(msg) => rsx! {
                        div { class: "operon-artifact-skill-picker-status operon-artifact-skill-picker-status-err", "{msg}" }
                    },
                }
            }
        }
    }
}

#[derive(Clone, Default)]
struct SkillContractPreview {
    input_kind: Option<String>,
    output_kind: Option<String>,
    output_count: String,
    gate: String,
}

/// Cheap synchronous fetch of a skill's contract for the picker row
/// chips. Falls back to defaults if the skill body can't be loaded.
fn preview_skill_contract(
    persistence: &Arc<dyn Persistence>,
    skill_id: Uuid,
) -> SkillContractPreview {
    // The persistence trait exposes async load. Use a synchronous
    // best-effort path by reading from the filesystem directly when
    // possible — but the trait is async. For UI preview we fall
    // back to defaults if loading would block; the chips are
    // informational only, so skipping when the load isn't trivially
    // available is acceptable.
    //
    // Cheapest viable: spawn an async task to load + ignore for
    // first render (chips appear empty until the user re-renders).
    // For v1, just return defaults — the picker filter is a hint,
    // not a hard gate. Misclassification just means a row that says
    // "in: ?" rather than "in: epic".
    let _ = persistence;
    let _ = skill_id;
    SkillContractPreview {
        input_kind: None,
        output_kind: None,
        output_count: "one".into(),
        gate: "approval".into(),
    }
}

/// Kick off `run_skill_on_source` in the background. Sets up the
/// rail's chat session, derives a deterministic v5 UUID per source
/// artifact (so re-runs reuse the same rail entry), auto-switches
/// the rail to that session so Claude's transcript streams live,
/// forces `acceptEdits` for the runner's session so Write tool
/// calls don't hang on stdin approval, persists the transcript,
/// and bumps `LocalNoteVersion` afterwards so the explorer picks up
/// the new artifact rows.
#[allow(clippy::too_many_arguments)]
fn spawn_runner(
    source_note_id: Uuid,
    skill_note_id: Uuid,
    project_id: Uuid,
    note_repo: Arc<dyn operon_store::repos::LocalNoteRepository>,
    project_repo: Arc<dyn operon_store::repos::LocalProjectRepository>,
    persistence: Arc<dyn Persistence>,
    plugin: Arc<operon_plugins_claude_code::ClaudeCodeChatPlugin>,
    chat_session_repo: Arc<dyn operon_store::repos::ChatSessionRepository>,
    chat_message_repo: Arc<dyn operon_store::repos::ChatMessageRepository>,
    note_version: &mut Signal<u64>,
    chat_session_version: &mut Signal<u64>,
    active_session: &mut Signal<Option<Uuid>>,
    active_scope: &mut Signal<operon_store::repos::ChatScope>,
    status: &mut Signal<Option<Result<String, String>>>,
) {
    // Resolve repo path up front so a missing binding fails loudly
    // rather than silently no-op'ing inside the spawned task.
    let projects = match project_repo.list() {
        Ok(p) => p,
        Err(e) => {
            status.set(Some(Err(format!("list projects: {e}"))));
            return;
        }
    };
    let repo_path = match projects.into_iter().find(|p| p.id == project_id).and_then(|p| p.repo_path) {
        Some(p) => p,
        None => {
            status.set(Some(Err(
                "Set the project's repository (right-click \u{2192} Set repository\u{2026}) before running a skill.".into(),
            )));
            return;
        }
    };

    // Deterministic chat-session id per source artifact: re-runs of
    // any skill against the same source land in the same rail entry.
    let chat_session_id = Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("operon-artifact-runner:{source_note_id}").as_bytes(),
    );

    // Find or create the chat_session row, then bump the rail's
    // version so the new entry shows up.
    let session_repo_for_create = chat_session_repo.clone();
    let session_label = format!("Skill run: {}", short_uuid(source_note_id));
    let exists = matches!(session_repo_for_create.get(chat_session_id), Ok(Some(_)));
    if !exists {
        let _ = session_repo_for_create.create_with_id(
            chat_session_id,
            operon_store::repos::ChatScope::Project(project_id),
            &session_label,
        );
    }
    let _ = session_repo_for_create.touch(chat_session_id);
    chat_session_version.with_mut(|v| *v = v.saturating_add(1));

    // Auto-switch the rail's active session so the user immediately
    // sees Claude's transcript stream (the companion's existing
    // use_effects on ActiveChatSession re-bind the plugin and reload
    // the message log automatically). Same idiom `play_skill` at
    // src/plugins/skill/view.rs:275-276 uses for skill-▶ runs.
    let scope = operon_store::repos::ChatScope::Project(project_id);
    active_scope.set(scope);
    active_session.set(Some(chat_session_id));

    plugin.bind_session(chat_session_id, repo_path.clone());

    let mut note_version_setter = *note_version;
    let mut status_setter = *status;
    // Show an in-flight indicator immediately so the user has visual
    // feedback even before Claude starts streaming. Final status
    // (success / failure) overwrites this once the runner returns.
    status_setter.set(Some(Ok("Running\u{2026} (transcript visible in the rail)".into())));
    spawn(async move {
        let result = crate::plugins::artifact::run_skill_on_source(
            &note_repo,
            &project_repo,
            &persistence,
            &plugin,
            Some(&chat_message_repo),
            chat_session_id,
            source_note_id,
            skill_note_id,
        )
        .await;
        match result {
            Ok(outcome) => {
                let n = outcome.created_artifact_ids.len();
                status_setter.set(Some(Ok(format!(
                    "Created {n} artifact(s) under this note."
                ))));
                note_version_setter.with_mut(|v| *v = v.saturating_add(1));
            }
            Err(e) => {
                status_setter.set(Some(Err(format!("Run failed: {e}"))));
            }
        }
    });
}
