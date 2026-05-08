//! `ArtifactFormatPlugin` — `format_id = "artifact"`. Renders an
//! artifact note with a status pill, action buttons (Approve /
//! Reject / Re-run), and the markdown body. The body editor itself
//! is plain `<textarea>` so the user can edit acceptance criteria
//! and other inline content; frontmatter mutations from the action
//! buttons go through `frontmatter::rewrite` so the user's edits in
//! the body aren't blown away.

use dioxus::prelude::*;
use operon_store::repos::{LocalNote, LocalNoteRepository, NoteKind};
use std::collections::HashMap;
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
    ArtifactRunState, ChatMessageRepo, ChatSessionRepo, ChatSessionVersion,
    ClaudeCodePluginCtx, ARTIFACT_RUN_STATE,
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
    // Pipeline gate: a downstream skill may run on this artifact only
    // when the artifact is Approved, or when it's a root seed (no
    // upstream parent — e.g. a user-authored Requirements note).
    // Pending / Rejected / Dirty children must be re-approved before
    // their own descendants can be regenerated. Mirrored at the
    // runtime layer in `runner::run_skill_on_source`.
    let is_root_seed = fm.source_artifact_id.is_none();
    let is_runnable_source = status == ArtifactStatus::Approved || is_root_seed;
    let run_button_title = if is_runnable_source {
        "Run a skill on this artifact to produce child artifacts (Epics, Features, etc.).".to_string()
    } else {
        format!(
            "Approve this {} first \u{2014} child skills are gated on parent approval.",
            fm.artifact_kind
                .as_ref()
                .map(|k| k.display_name().to_lowercase())
                .unwrap_or_else(|| "artifact".into())
        )
    };
    // Phase E: read run state from the global `ARTIFACT_RUN_STATE`
    // map. The map is keyed on the runner's deterministic
    // chat_session_id (so the companion's loader and this view
    // share one entry), derived from the source artifact id via
    // `chat_session_id_for_source`. Reading the GlobalSignal here
    // subscribes the artifact view to map updates, so the status
    // pill ticks as the runner progresses (Running → Done /
    // Failed) without scope-ownership warnings.
    let run_state_view: Option<ArtifactRunState> = source_uuid
        .map(chat_session_id_for_source)
        .and_then(|sid| ARTIFACT_RUN_STATE.read().get(&sid).cloned());
    // Subscribe the artifact view to the cascade-state map so the
    // Play button morphs to ⏹ Stop while a cascade rooted on this
    // artifact is in flight, and the inline status line renders the
    // current step.
    let cascade_state_view: Option<crate::shell::companion_state::CascadePhase> = source_uuid
        .and_then(|sid| crate::shell::companion_state::CASCADE_STATE.read().get(&sid).cloned());
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
                        if let Some(uuid) = source_uuid {
                            ReviseButton { artifact_id: uuid }
                        }
                        if status == ArtifactStatus::Dirty {
                            if let (Some(parent), Some(skill)) =
                                (fm.source_artifact_id, fm.source_skill_id)
                            {
                                RerunButton { parent_id: parent, skill_id: skill }
                            }
                        }
                        button {
                            r#type: "button",
                            class: "operon-artifact-run-skill",
                            "data-testid": "artifact-run-skill",
                            disabled: !is_runnable_source,
                            title: "{run_button_title}",
                            onclick: move |_| {
                                if is_runnable_source {
                                    picker_open.with_mut(|v| *v = !*v);
                                }
                            },
                            if *picker_open.read() { "Hide skills" } else { "Run skill\u{2026}" }
                        }
                        if let Some(uuid) = source_uuid {
                            if is_runnable_source {
                                CascadePlayButton { root_artifact_id: uuid }
                            }
                        }
                    }
                }
            }
            if *picker_open.read() {
                if let Some(src_uuid) = source_uuid {
                    SkillPickerPanel {
                        source_note_id: src_uuid,
                        source_artifact_kind: source_kind_str.clone(),
                        source_body: props.content.clone(),
                        on_dismiss: Callback::new(move |_| picker_open.set(false)),
                    }
                }
            }
            // Inline run-status row, visible regardless of whether
            // the picker is open. Empty before the first run;
            // shows the live state (Running / Done / Failed) keyed
            // off the global run-state map.
            match run_state_view {
                None => rsx! {},
                Some(ArtifactRunState::Running) => rsx! {
                    div {
                        class: "operon-artifact-run-status operon-artifact-run-status-ok",
                        "data-testid": "artifact-run-status",
                        "Running\u{2026} (transcript visible in the rail)"
                    }
                },
                Some(ArtifactRunState::Done { artifact_count }) => rsx! {
                    div {
                        class: "operon-artifact-run-status operon-artifact-run-status-ok",
                        "data-testid": "artifact-run-status",
                        "Created {artifact_count} artifact(s) under this note."
                    }
                },
                Some(ArtifactRunState::Failed { reason }) => rsx! {
                    div {
                        class: "operon-artifact-run-status operon-artifact-run-status-err",
                        "data-testid": "artifact-run-status",
                        "Run failed: {reason}"
                    }
                },
            }
            // Cascade-state row: visible while a Play-cascade rooted
            // on this artifact is running or after it finished. Sits
            // right under the per-skill run-status so the user sees
            // both: "current Claude turn …" + "cascade level N".
            match cascade_state_view {
                None => rsx! {},
                Some(crate::shell::companion_state::CascadePhase::Running { level, .. }) => rsx! {
                    div {
                        class: "operon-artifact-run-status operon-artifact-run-status-ok",
                        "data-testid": "artifact-cascade-status",
                        "Cascade running\u{2026} (level {level})"
                    }
                },
                Some(crate::shell::companion_state::CascadePhase::Completed { artifacts_produced }) => rsx! {
                    div {
                        class: "operon-artifact-run-status operon-artifact-run-status-ok",
                        "data-testid": "artifact-cascade-status",
                        "Cascade completed \u{2014} produced {artifacts_produced} artifact(s)."
                    }
                },
                Some(crate::shell::companion_state::CascadePhase::Cancelled) => rsx! {
                    div {
                        class: "operon-artifact-run-status",
                        "data-testid": "artifact-cascade-status",
                        "Cascade cancelled."
                    }
                },
                Some(crate::shell::companion_state::CascadePhase::Failed { reason }) => rsx! {
                    div {
                        class: "operon-artifact-run-status operon-artifact-run-status-err",
                        "data-testid": "artifact-cascade-status",
                        "Cascade failed: {reason}"
                    }
                },
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
    /// The artifact's current in-memory body (full content including
    /// frontmatter). Forwarded into `spawn_runner` so it can flush
    /// the latest status to disk before the runner reads it back —
    /// closes the race between Approve (debounced disk save) and Run.
    source_body: String,
    on_dismiss: Callback<()>,
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
                            let mut note_version_setter = note_version;
                            let mut chat_session_version_setter = chat_session_version;
                            let mut active_session_setter = active_session;
                            let mut active_scope_setter = active_scope;
                            let on_dismiss = props.on_dismiss;
                            let source_id = props.source_note_id;
                            let source_body_for_run = props.source_body.clone();
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
                                                Some(source_body_for_run.clone()),
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
///
/// `source_body_inflight` is the artifact's current in-memory body —
/// when present, we pre-flush it to disk before launching the runner
/// so the runtime gate sees the latest status (closes the race
/// between the editor's debounced save scheduler and clicking Run
/// immediately after Approve). Pass `None` for call sites that
/// don't have the in-memory snapshot (e.g. the Re-run button on a
/// child artifact, where we run against the parent's already-on-disk
/// body).
#[allow(clippy::too_many_arguments)]
fn spawn_runner(
    source_note_id: Uuid,
    skill_note_id: Uuid,
    project_id: Uuid,
    source_body_inflight: Option<String>,
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
) {
    // Deterministic chat-session id per source artifact: re-runs of
    // any skill against the same source land in the same rail entry.
    // Computed up front so all `ARTIFACT_RUN_STATE` writes use it as
    // the map key — that's the same key the companion uses to look
    // up "is this session running" for its loader / streaming
    // surface.
    let chat_session_id = chat_session_id_for_source(source_note_id);

    // Resolve repo path up front so a missing binding fails loudly
    // rather than silently no-op'ing inside the spawned task.
    let projects = match project_repo.list() {
        Ok(p) => p,
        Err(e) => {
            ARTIFACT_RUN_STATE.with_mut(|m| {
                m.insert(
                    chat_session_id,
                    ArtifactRunState::Failed { reason: format!("list projects: {e}") },
                );
            });
            return;
        }
    };
    let repo_path = match projects.into_iter().find(|p| p.id == project_id).and_then(|p| p.repo_path) {
        Some(p) => p,
        None => {
            ARTIFACT_RUN_STATE.with_mut(|m| {
                m.insert(
                    chat_session_id,
                    ArtifactRunState::Failed {
                        reason: "Set the project's repository (right-click \u{2192} Set repository\u{2026}) before running a skill.".into(),
                    },
                );
            });
            return;
        }
    };

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

    // Show the in-flight indicator immediately. Keyed on
    // chat_session_id so the companion's "Claude is thinking…"
    // loader (which knows the active chat session, not the source
    // artifact) can also subscribe to the same state.
    ARTIFACT_RUN_STATE.with_mut(|m| {
        m.insert(chat_session_id, ArtifactRunState::Running);
    });
    // The post-completion `LocalNoteVersion` bump now goes through
    // `LOCAL_NOTE_VERSION` (a GlobalSignal) — see the bump call below
    // — so the `__copy_value_hoisted` warning is gone. The
    // component-scope Signal is bridged back via a `use_effect` in
    // `desktop.rs::Workspace`, so existing readers don't need to
    // change. The local binding is kept only so `spawn_forever`'s
    // `move` closure compiles unchanged; the value isn't actually used.
    let note_version_setter = *note_version;
    // `spawn_forever` (NOT plain `spawn`) attaches the task to the
    // root scope. We need this because the SkillPickerPanel that
    // owns this click handler is dismissed immediately after via
    // `on_dismiss.call(())` — with plain `spawn`, the task would be
    // attached to the picker's scope and dropped before the
    // executor polls it.
    dioxus::core::spawn_forever(async move {
        // Pre-flush: when the caller supplied the artifact's current
        // in-memory body, persist it before invoking the runner so the
        // runtime gate sees the latest status. Without this, an
        // immediate Run after Approve sees stale `pending` bytes
        // (the editor's save scheduler is debounced).
        if let Some(body) = source_body_inflight {
            if let Err(e) = persistence
                .save(&source_note_id.to_string(), body.as_bytes())
                .await
            {
                ARTIFACT_RUN_STATE.with_mut(|m| {
                    m.insert(
                        chat_session_id,
                        ArtifactRunState::Failed {
                            reason: format!("pre-flush failed: {e}"),
                        },
                    );
                });
                return;
            }
        }
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
        // Both Ok/Err writes go to the GlobalSignal map, so they
        // don't trigger `__copy_value_hoisted` warnings. The
        // artifact view subscribes to the map and re-renders the
        // status pill automatically.
        match result {
            Ok(outcome) => {
                let n = outcome.created_artifact_ids.len();
                ARTIFACT_RUN_STATE.with_mut(|m| {
                    m.insert(
                        chat_session_id,
                        ArtifactRunState::Done { artifact_count: n },
                    );
                });
                // Refresh the explorer so the new artifact rows
                // appear under the source. Writes to the GlobalSignal
                // (safe from `spawn_forever`'s detached scope); the
                // bridge effect in desktop.rs::Workspace mirrors the
                // bump back into the component-scope `LocalNoteVersion`
                // Signal that explorer readers subscribe to.
                let _ = note_version_setter; // bridge handles the local Signal
                crate::shell::companion_state::LOCAL_NOTE_VERSION
                    .with_mut(|v| *v = v.saturating_add(1));
            }
            Err(e) => {
                ARTIFACT_RUN_STATE.with_mut(|m| {
                    m.insert(
                        chat_session_id,
                        ArtifactRunState::Failed { reason: format!("{e}") },
                    );
                });
            }
        }
    });
}

/// Public derivation of the deterministic chat-session UUID for an
/// artifact runner keyed on the source artifact's id. Used by both
/// `spawn_runner` (to bind the rail entry + ARTIFACT_RUN_STATE) and
/// the artifact view's render (to look up its current run state by
/// the same key the companion uses).
pub fn chat_session_id_for_source(source_note_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("operon-artifact-runner:{source_note_id}").as_bytes(),
    )
}

/// Re-run the producing skill against the parent artifact so this
/// (Dirty) artifact gets regenerated from current parent content.
/// Reuses `spawn_runner` plus the runner-side dedup so the existing
/// note row is updated in place rather than a duplicate sibling
/// being created. Visible only when status == Dirty AND the
/// artifact's frontmatter still carries source_artifact_id +
/// source_skill_id (i.e. it was produced by a skill, not user-authored).
#[derive(Props, Clone, PartialEq)]
struct RerunButtonProps {
    parent_id: Uuid,
    skill_id: Uuid,
}

#[component]
fn RerunButton(props: RerunButtonProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let ClaudeCodePluginCtx(plugin) = use_context();
    let ChatSessionRepo(chat_session_repo) = use_context();
    let ChatMessageRepo(chat_message_repo) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let ChatSessionVersion(chat_session_version) = use_context();
    let crate::shell::companion_state::ActiveChatSession(active_session) = use_context();
    let crate::shell::companion_state::ActiveChatScope(active_scope) = use_context();
    let parent_id = props.parent_id;
    let skill_id = props.skill_id;

    rsx! {
        button {
            r#type: "button",
            class: "operon-artifact-rerun",
            "data-testid": "artifact-rerun",
            title: "Re-run the producing skill against the parent — overwrites this artifact (siblings produced by the same skill may also be regenerated).",
            onclick: move |_| {
                let project_id = match note_repo.find_project_for_note(parent_id) {
                    Ok(Some(p)) => p,
                    _ => {
                        tracing::warn!(
                            target: "operon::artifact",
                            "rerun: parent {parent_id} has no project, skipping"
                        );
                        return;
                    }
                };
                let mut note_version_setter = note_version;
                let mut chat_session_version_setter = chat_session_version;
                let mut active_session_setter = active_session;
                let mut active_scope_setter = active_scope;
                spawn_runner(
                    parent_id,
                    skill_id,
                    project_id,
                    None,
                    note_repo.clone(),
                    project_repo.clone(),
                    persistence.clone(),
                    plugin.clone(),
                    chat_session_repo.clone(),
                    chat_message_repo.clone(),
                    &mut note_version_setter,
                    &mut chat_session_version_setter,
                    &mut active_session_setter,
                    &mut active_scope_setter,
                );
            },
            "Re-run"
        }
    }
}

/// Pipeline-revision affordance: walks every Approved descendant of
/// this artifact in the note tree and flips them to Dirty so the user
/// knows a re-run is needed against the now-edited parent. Pulls the
/// note repo + persistence + version-bump signal from context, so the
/// caller (the editable artifact action row) only passes the artifact
/// id.
#[derive(Props, Clone, PartialEq)]
struct ReviseButtonProps {
    artifact_id: Uuid,
}

#[component]
fn ReviseButton(props: ReviseButtonProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let artifact_id = props.artifact_id;

    rsx! {
        button {
            r#type: "button",
            class: "operon-artifact-revise",
            "data-testid": "artifact-revise",
            title: "Cascade: mark every approved descendant as Dirty so they get re-run against this revised parent.",
            onclick: move |_| {
                let note_repo = note_repo.clone();
                let persistence = persistence.clone();
                let _ = note_version; // bridge effect handles the local Signal
                dioxus::core::spawn_forever(async move {
                    match mark_descendants_dirty(&note_repo, &persistence, artifact_id).await {
                        Ok(n) => {
                            tracing::info!(
                                target: "operon::artifact",
                                "revise: marked {n} descendant(s) of {artifact_id} dirty"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "operon::artifact",
                                "revise walk failed for {artifact_id}: {e}"
                            );
                        }
                    }
                    // GlobalSignal write from spawn_forever's detached
                    // scope (the bridge in desktop.rs::Workspace mirrors
                    // this back into the component-scope Signal).
                    crate::shell::companion_state::LOCAL_NOTE_VERSION
                        .with_mut(|v| *v = v.saturating_add(1));
                });
            },
            "Revise"
        }
    }
}

/// Walk every Artifact descendant of `root_id` in the note tree and
/// flip its status from Approved → Dirty. Returns the number of rows
/// mutated. Errors are best-effort: a single load/save failure
/// doesn't abort the walk.
async fn mark_descendants_dirty(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    root_id: Uuid,
) -> Result<usize, String> {
    let project_id = note_repo
        .find_project_for_note(root_id)
        .map_err(|e| format!("find_project: {e}"))?
        .ok_or_else(|| format!("note {root_id} has no project"))?;
    let all = note_repo
        .list_for_project(project_id)
        .map_err(|e| format!("list_for_project: {e}"))?;

    // Index parent → children so the walk is O(n) regardless of depth.
    let mut by_parent: HashMap<Uuid, Vec<&LocalNote>> = HashMap::new();
    for n in &all {
        if let Some(p) = n.parent_id {
            by_parent.entry(p).or_default().push(n);
        }
    }

    // BFS descendants (only Artifact-kind notes — we don't touch
    // Markdown / Skill / Workflow children that happen to live under
    // an artifact).
    let mut descendants: Vec<Uuid> = Vec::new();
    let mut stack = vec![root_id];
    while let Some(id) = stack.pop() {
        if let Some(kids) = by_parent.get(&id) {
            for k in kids {
                if matches!(k.kind, NoteKind::Artifact) {
                    descendants.push(k.id);
                    stack.push(k.id);
                }
            }
        }
    }

    let mut changed = 0usize;
    for id in descendants {
        let bytes = match persistence.load(&id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut fm = parse(&body);
        if fm.status == ArtifactStatus::Approved {
            fm.status = ArtifactStatus::Dirty;
            let new_body = rewrite(&body, &fm);
            if persistence
                .save(&id.to_string(), new_body.as_bytes())
                .await
                .is_ok()
            {
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// Autonomous cascade control: a green ▶ Play button that fans out
/// the SDLC pipeline from this artifact, plus a ▾ chevron that opens
/// a stages-dropdown panel listing every project skill as a checkbox.
/// While a cascade is in flight (CASCADE_STATE has a Running entry
/// keyed on this artifact), the Play button morphs into a red ⏹ Stop
/// button that cancels the cooperative `CancellationToken`.
#[derive(Props, Clone, PartialEq)]
struct CascadePlayButtonProps {
    root_artifact_id: Uuid,
}

#[component]
fn CascadePlayButton(props: CascadePlayButtonProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let ClaudeCodePluginCtx(plugin) = use_context();
    let ChatSessionRepo(chat_session_repo) = use_context();
    let ChatMessageRepo(chat_message_repo) = use_context();
    let LocalNoteVersion(note_version) = use_context();
    let ChatSessionVersion(chat_session_version) = use_context();
    let crate::shell::companion_state::ActiveChatSession(active_session) = use_context();
    let crate::shell::companion_state::ActiveChatScope(active_scope) = use_context();

    let root_id = props.root_artifact_id;
    let mut stages_open = use_signal(|| false);

    // Are we currently cascading? Subscribes via the global map read
    // above; re-renders on every CASCADE_STATE bump.
    let is_running = matches!(
        crate::shell::companion_state::CASCADE_STATE.read().get(&root_id),
        Some(crate::shell::companion_state::CascadePhase::Running { .. })
    );

    rsx! {
        button {
            r#type: "button",
            class: if is_running { "operon-artifact-cascade-stop" } else { "operon-artifact-cascade-play" },
            "data-testid": "artifact-cascade-play",
            title: if is_running {
                "Stop the cascade at the next skill boundary."
            } else {
                "Run the entire SDLC pipeline from this artifact \u{2014} every produced child auto-approves."
            },
            onclick: {
                let note_repo = note_repo.clone();
                let project_repo = project_repo.clone();
                let persistence = persistence.clone();
                let plugin = plugin.clone();
                let chat_session_repo = chat_session_repo.clone();
                let chat_message_repo = chat_message_repo.clone();
                let mut note_version_setter = note_version;
                let mut chat_session_version_setter = chat_session_version;
                let mut active_session_setter = active_session;
                let mut active_scope_setter = active_scope;
                move |_| {
                    if is_running {
                        // Cancel: tell the orchestrator's loop to bail.
                        if let Some(tok) = crate::shell::companion_state::CASCADE_CANCEL
                            .read()
                            .get(&root_id)
                            .cloned()
                        {
                            tok.cancel();
                        }
                    } else {
                        spawn_cascade(
                            root_id,
                            note_repo.clone(),
                            project_repo.clone(),
                            persistence.clone(),
                            plugin.clone(),
                            chat_session_repo.clone(),
                            chat_message_repo.clone(),
                            &mut note_version_setter,
                            &mut chat_session_version_setter,
                            &mut active_session_setter,
                            &mut active_scope_setter,
                        );
                    }
                }
            },
            if is_running { "\u{23F9} Stop" } else { "\u{25B6} Play" }
        }
        button {
            r#type: "button",
            class: "operon-artifact-cascade-stages-toggle",
            "data-testid": "artifact-cascade-stages-toggle",
            title: "Configure which pipeline stages run when you click Play.",
            onclick: move |_| stages_open.with_mut(|v| *v = !*v),
            "\u{25BE}"
        }
        if *stages_open.read() {
            // Click-outside dismissal: a transparent fixed-position
            // backdrop sits below the dropdown's z-index, so any
            // click outside the panel closes it. Panel itself stops
            // click propagation so checkbox clicks don't bubble up.
            div {
                class: "operon-artifact-cascade-stages-backdrop",
                "data-testid": "artifact-cascade-stages-backdrop",
                onclick: move |_| stages_open.set(false),
            }
            CascadeStagesDropdown {
                root_artifact_id: root_id,
                on_dismiss: Callback::new(move |_| stages_open.set(false)),
            }
        }
    }
}

/// Inline panel rendered under the Play button: lists every project
/// skill as a checkbox row, in pipeline order (Requirements → Epic →
/// Feature → … → Summary inferred from `input_kind`/`output_kind`).
/// Persists toggles to `<repo>/.operon/cascade-stages.json` via the
/// `cascade::stages_sidecar` helpers.
#[derive(Props, Clone, PartialEq)]
struct CascadeStagesDropdownProps {
    root_artifact_id: Uuid,
    on_dismiss: Callback<()>,
}

#[component]
fn CascadeStagesDropdown(props: CascadeStagesDropdownProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();

    // Resolve project + repo path. Without a repo path we can't store
    // the sidecar, so the panel renders read-only with a hint.
    let project_id = match note_repo.find_project_for_note(props.root_artifact_id) {
        Ok(Some(p)) => p,
        _ => {
            return rsx! {
                div { class: "operon-artifact-cascade-stages-panel",
                    "data-testid": "artifact-cascade-stages-panel",
                    onclick: move |evt: dioxus::events::MouseEvent| {
                        evt.stop_propagation();
                    },
                    div { class: "operon-artifact-skill-picker-empty",
                        "This note has no project bound \u{2014} can't list skills."
                    }
                }
            };
        }
    };
    let repo_path: Option<std::path::PathBuf> = project_repo
        .list()
        .ok()
        .and_then(|all| all.into_iter().find(|p| p.id == project_id))
        .and_then(|p| p.repo_path);

    // List project skills — same source as the regular Run-skill picker.
    let project_notes = note_repo.list_for_project(project_id).unwrap_or_default();
    let mut skill_rows: Vec<operon_store::repos::LocalNote> = project_notes
        .into_iter()
        .filter(|n| matches!(n.kind, operon_store::repos::NoteKind::Skill))
        .collect();
    skill_rows.sort_by(|a, b| a.title.cmp(&b.title));
    let all_ids: std::collections::HashSet<Uuid> = skill_rows.iter().map(|n| n.id).collect();

    // Load enabled set (sidecar or "everything" if absent).
    let enabled_initial = match repo_path.as_ref() {
        Some(p) => crate::plugins::artifact::cascade::stages_sidecar::resolve_or_all(p, &all_ids),
        None => all_ids.clone(),
    };
    let mut enabled = use_signal(|| enabled_initial);

    rsx! {
        div { class: "operon-artifact-cascade-stages-panel",
            "data-testid": "artifact-cascade-stages-panel",
            onclick: move |evt: dioxus::events::MouseEvent| {
                evt.stop_propagation();
            },
            div { class: "operon-artifact-skill-picker-header",
                span { "Pipeline stages \u{2014} uncheck to skip" }
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
            if skill_rows.is_empty() {
                div { class: "operon-artifact-skill-picker-empty",
                    "No skill notes in this project yet \u{2014} import the seed-skills folder first."
                }
            } else {
                ul { class: "operon-artifact-skill-picker-list",
                    for skill in skill_rows.iter() {
                        {
                            let skill_id = skill.id;
                            let skill_title = skill.title.clone();
                            let checked = enabled.read().contains(&skill_id);
                            let repo_path_for_save = repo_path.clone();
                            rsx! {
                                li {
                                    key: "{skill_id}",
                                    class: "operon-artifact-skill-picker-item",
                                    label {
                                        style: "display: flex; align-items: center; gap: 0.5rem; cursor: pointer;",
                                        input {
                                            r#type: "checkbox",
                                            checked,
                                            onchange: move |evt| {
                                                let now_checked = evt.checked();
                                                enabled.with_mut(|set| {
                                                    if now_checked { set.insert(skill_id); }
                                                    else { set.remove(&skill_id); }
                                                });
                                                if let Some(path) = repo_path_for_save.as_ref() {
                                                    let snapshot = enabled.read().clone();
                                                    if let Err(e) =
                                                        crate::plugins::artifact::cascade::stages_sidecar::save(path, &snapshot)
                                                    {
                                                        tracing::warn!(
                                                            target: "operon::cascade",
                                                            "stages_sidecar::save failed: {e}"
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        span { "{skill_title}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if repo_path.is_none() {
                div { class: "operon-artifact-skill-picker-empty",
                    "Bind a repository to this project to persist your selection across sessions."
                }
            }
        }
    }
}

/// Spawn the autonomous cascade orchestrator in the background.
/// Mirrors `spawn_runner`'s setup (binds the rail's chat session,
/// auto-switches the rail's active session so transcripts stream
/// live, forces `acceptEdits` on the runner's session) but invokes
/// `cascade::run_cascade` instead of a single `run_skill_on_source`.
///
/// Cancellation is wired through `CASCADE_CANCEL` keyed on the root
/// artifact id — the Play button writes a fresh `CancellationToken`
/// here, the orchestrator polls between skill boundaries, the Stop
/// button calls `.cancel()` on the entry.
#[allow(clippy::too_many_arguments)]
fn spawn_cascade(
    root_artifact_id: Uuid,
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
) {
    // Resolve project + repo path up front so a missing binding fails
    // loudly rather than silently no-op'ing inside the spawned task.
    let project_id = match note_repo.find_project_for_note(root_artifact_id) {
        Ok(Some(p)) => p,
        _ => {
            crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                m.insert(
                    root_artifact_id,
                    crate::shell::companion_state::CascadePhase::Failed {
                        reason: "root artifact has no project".into(),
                    },
                );
            });
            return;
        }
    };
    let projects = match project_repo.list() {
        Ok(p) => p,
        Err(e) => {
            crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                m.insert(
                    root_artifact_id,
                    crate::shell::companion_state::CascadePhase::Failed {
                        reason: format!("list projects: {e}"),
                    },
                );
            });
            return;
        }
    };
    let project = match projects.into_iter().find(|p| p.id == project_id) {
        Some(p) => p,
        None => {
            crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                m.insert(
                    root_artifact_id,
                    crate::shell::companion_state::CascadePhase::Failed {
                        reason: format!("project {project_id} not found"),
                    },
                );
            });
            return;
        }
    };
    let repo_path = match project.repo_path.clone() {
        Some(p) => p,
        None => {
            crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                m.insert(
                    root_artifact_id,
                    crate::shell::companion_state::CascadePhase::Failed {
                        reason: "Set the project's repository before running a cascade.".into(),
                    },
                );
            });
            return;
        }
    };

    // Resolve enabled-skill set: read sidecar, or fall back to all
    // project skills enabled.
    let all_skill_ids: std::collections::HashSet<Uuid> = note_repo
        .list_for_project(project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| matches!(n.kind, operon_store::repos::NoteKind::Skill))
        .map(|n| n.id)
        .collect();
    let enabled =
        crate::plugins::artifact::cascade::stages_sidecar::resolve_or_all(&repo_path, &all_skill_ids);

    // Bind the rail to a deterministic cascade session so transcripts
    // for each skill run inside the cascade land in the same rail
    // entry as a per-source run would. We reuse the per-source
    // session id derivation inside the orchestrator (each individual
    // skill run keys on its own source artifact), and bind a separate
    // cascade-level session here mainly so the project's chat
    // surface has a labeled entry.
    let cascade_session_id = chat_session_id_for_cascade(root_artifact_id);
    let session_label = format!("Cascade: {}", short_uuid(root_artifact_id));
    let exists = matches!(chat_session_repo.get(cascade_session_id), Ok(Some(_)));
    if !exists {
        let _ = chat_session_repo.create_with_id(
            cascade_session_id,
            operon_store::repos::ChatScope::Project(project_id),
            &session_label,
        );
    }
    let _ = chat_session_repo.touch(cascade_session_id);
    chat_session_version.with_mut(|v| *v = v.saturating_add(1));
    let scope = operon_store::repos::ChatScope::Project(project_id);
    active_scope.set(scope);
    active_session.set(Some(cascade_session_id));
    plugin.bind_session(cascade_session_id, repo_path.clone());

    // Cancellation handle — Stop button reads this map to cancel.
    let cancel = tokio_util::sync::CancellationToken::new();
    crate::shell::companion_state::CASCADE_CANCEL.with_mut(|m| {
        m.insert(root_artifact_id, cancel.clone());
    });

    // Initial Running entry so the artifact view's Play button morphs
    // immediately, before the first skill kicks off.
    crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
        m.insert(
            root_artifact_id,
            crate::shell::companion_state::CascadePhase::Running {
                artifact_id: root_artifact_id,
                skill_id: Uuid::nil(),
                level: 0,
            },
        );
    });

    // Resolve a human title for the cascade workflow note; defaults
    // to the artifact's UUID short-form when the row can't be loaded.
    let root_title = note_repo
        .list_for_project(project_id)
        .ok()
        .and_then(|all| all.into_iter().find(|n| n.id == root_artifact_id))
        .map(|n| n.title)
        .unwrap_or_else(|| short_uuid(root_artifact_id));
    let (graph_note_id, graph_was_created) =
        match crate::plugins::artifact::cascade_graph::ensure_cascade_workflow_note(
            &note_repo,
            project_id,
            &root_title,
        ) {
            Ok((id, was_created)) => (Some(id), was_created),
            Err(e) => {
                tracing::warn!(
                    target: "operon::cascade",
                    "could not create Cascade workflow note: {e}"
                );
                (None, false)
            }
        };
    // Seed a fresh Cascade-for-Requirements workflow with the natural
    // BA→SA→SDE skill pipeline so the editor opens to a runnable
    // 7-node DAG instead of an empty canvas. Read the root artifact's
    // kind here (sync — `read_kind` is async, so we re-load
    // synchronously via persistence's blocking guarantee for tiny
    // bodies isn't safe; instead, defer the kind check to the
    // spawn_forever async block below).
    let seed_graph_note_id = if graph_was_created { graph_note_id } else { None };

    let note_version_setter = *note_version;
    dioxus::core::spawn_forever(async move {
        if let Some(gid) = seed_graph_note_id {
            let kind = crate::plugins::artifact::cascade::read_kind(&persistence, root_artifact_id)
                .await;
            if kind.as_deref() == Some("requirements") {
                if let Err(e) = crate::plugins::artifact::cascade_graph::seed_natural_pipeline(
                    &note_repo,
                    &persistence,
                    project_id,
                    gid,
                )
                .await
                {
                    tracing::warn!(
                        target: "operon::cascade",
                        "seed_natural_pipeline failed: {e}"
                    );
                }
            }
        }
        let mut writer = match graph_note_id {
            Some(gid) => Some(
                crate::plugins::artifact::cascade_graph::CascadeGraphWriter::new_or_load(
                    gid,
                    &persistence,
                )
                .await,
            ),
            None => None,
        };
        let result = crate::plugins::artifact::cascade::run_cascade(
            &note_repo,
            &project_repo,
            &persistence,
            &plugin,
            &chat_message_repo,
            project_id,
            root_artifact_id,
            enabled,
            cancel.clone(),
            writer.as_mut(),
        )
        .await;
        match result {
            Ok(crate::plugins::artifact::cascade::CascadeOutcome::Completed {
                artifacts_produced,
            }) => {
                crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                    m.insert(
                        root_artifact_id,
                        crate::shell::companion_state::CascadePhase::Completed {
                            artifacts_produced,
                        },
                    );
                });
            }
            Ok(crate::plugins::artifact::cascade::CascadeOutcome::Cancelled { .. }) => {
                crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                    m.insert(
                        root_artifact_id,
                        crate::shell::companion_state::CascadePhase::Cancelled,
                    );
                });
            }
            Err(e) => {
                crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                    m.insert(
                        root_artifact_id,
                        crate::shell::companion_state::CascadePhase::Failed {
                            reason: format!("{e}"),
                        },
                    );
                });
            }
        }
        // Drop the cancel token from the registry so a subsequent run
        // creates a fresh token.
        crate::shell::companion_state::CASCADE_CANCEL.with_mut(|m| {
            m.remove(&root_artifact_id);
        });
        // Bump the GlobalSignal — `spawn_forever` runs in the virtual
        // root scope, so writes to the component-scope Signal here
        // would be silently dropped.
        let _ = note_version_setter;
        crate::shell::companion_state::LOCAL_NOTE_VERSION
            .with_mut(|v| *v = v.saturating_add(1));
    });
}

/// Deterministic v5 UUID for the cascade-level rail entry. Distinct
/// namespace from `chat_session_id_for_source` so cascade sessions
/// don't collide with single-skill sessions on the same artifact.
fn chat_session_id_for_cascade(root: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("operon-artifact-cascade:{root}").as_bytes(),
    )
}
