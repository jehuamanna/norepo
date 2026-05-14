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

use crate::editor::LanguageDescriptor;
use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::LocalNoteVersion;
use crate::persistence::Persistence;
use crate::plugins::artifact::frontmatter::{
    parse, rewrite, ArtifactFrontmatter, ArtifactKind, ArtifactStatus,
};
use crate::plugins::markdown::MarkdownView;
use crate::shell::clarification_prompt::{
    parse_clarification, ClarificationAnswer, ClarificationPanel,
};
use crate::shell::companion_state::{
    ArtifactRunState, ChatMessageRepo, ChatSessionRepo, ChatSessionVersion,
    ClaudeCodePluginCtx, ARTIFACT_RUN_STATE,
};
use crate::shell::editor_host::{MonacoChannel, MonacoEditorHost};

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

    // Status-change buttons (Approve / Reject / Mark dirty) write
    // straight to disk via `Persistence::save` so the new status
    // sticks immediately, without waiting on the user to press
    // Ctrl+S. Necessary in two cases:
    //   - View mode: `on_change` is `None` here (the artifact-plugin's
    //     `render` doesn't supply one), so the legacy on-change-only
    //     path silently dropped the status flip.
    //   - Edit mode with `manual_save` tabs: `on_change` updates the
    //     in-memory tab but the SaveScheduler short-circuits for
    //     manual-save tabs, so the bytes never reached disk until
    //     Ctrl+S.
    // We still call `on_change` afterwards so the open editor tab's
    // visible content reflects the patched body without a reload.
    let persistence_for_status: Arc<dyn Persistence> = use_context();
    let note_id_for_status = props.note_id.clone();
    let approve = {
        let content = content_for_actions.clone();
        let persistence = persistence_for_status.clone();
        let note_id = note_id_for_status.clone();
        move |_| save_status_change(
            &persistence,
            &note_id,
            &content,
            ArtifactStatus::Approved,
            on_change,
        )
    };
    let reject = {
        let content = content_for_actions.clone();
        let persistence = persistence_for_status.clone();
        let note_id = note_id_for_status.clone();
        move |_| save_status_change(
            &persistence,
            &note_id,
            &content,
            ArtifactStatus::Rejected,
            on_change,
        )
    };
    let mark_dirty = {
        let content = content_for_actions.clone();
        let persistence = persistence_for_status.clone();
        let note_id = note_id_for_status.clone();
        move |_| save_status_change(
            &persistence,
            &note_id,
            &content,
            ArtifactStatus::Dirty,
            on_change,
        )
    };

    let body_editable = props.edit && on_change.is_some();
    let monaco_body = body_only.clone();
    let fm_for_monaco = fm.clone();
    let content_for_monaco = content_for_actions.clone();

    // Revision dropdown state. `None` (default) = Current — the head
    // body editable as today. `Some(i)` (where `i >= 1`) = view the
    // i-th entry returned by `revisions::parse_revisions` (the
    // 0-index is always "Current", surfaced as `None` here). Selecting
    // a prior revision swaps the body conditional to read-only
    // `MarkdownView` of that revision's content.
    let revisions =
        crate::plugins::artifact::revisions::parse_revisions(&body_only);
    let mut selected_revision: Signal<Option<usize>> = use_signal(|| None);
    let active_revision_idx = *selected_revision.read();
    let viewing_prior_revision = active_revision_idx
        .map(|i| i >= 1 && i < revisions.len())
        .unwrap_or(false);
    let prior_revision_body: Option<String> = if viewing_prior_revision {
        active_revision_idx.and_then(|i| revisions.get(i).map(|r| r.body.clone()))
    } else {
        None
    };
    // Plans-Phase-9-monaco-desktop (rev 1): signal sink for the
    // MonacoChannel handle. Populated once the editor mounts; used by
    // the link picker (Task #3) to splice picked targets at the
    // current caret. Wired here so we don't have to re-touch the
    // editor mount when the picker lands.
    let monaco_channel: Signal<Option<MonacoChannel>> = use_signal(|| None);
    // Open-state for the link picker (Cmd+K linkpicker action).
    let mut link_picker_open: Signal<bool> = use_signal(|| false);
    let mut image_picker_open: Signal<bool> = use_signal(|| false);
    // Repos for the drop handler — only consumed on desktop, where
    // the explorer provides them via context. The artifact view is
    // mounted inside `LocalShell` so these always resolve when we
    // reach the body-editable branch.
    let LocalNoteRepo(drop_note_repo) = use_context();
    let LocalProjectRepo(drop_project_repo) = use_context();
    let crate::local_mode::ui::DragSession(drag_session) = use_context();
    // Persistence is fetched at component init so the clarification
    // submit handler can mutate sibling artifacts (resolution
    // targets) without re-pulling context inside a closure. Other
    // sub-components (`CascadePlayButton`, etc.) also pull it via
    // `use_context()` — the underlying Arc is cheap to clone.
    let persistence_for_clarification: Arc<dyn Persistence> = use_context();

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
    // Show the labeled "Generate Cascade" button on the project root —
    // either a `MasterRequirement` (updated chain) or a `Requirements`
    // artifact (legacy chain). The icon Play button is restricted to
    // the same set so every cascade starts from the documented project
    // root and so intermediate artifacts don't expose a per-node Play
    // that would re-trigger work upstream.
    let is_master_requirement =
        matches!(fm.artifact_kind, Some(ArtifactKind::MasterRequirement));
    let is_task = matches!(fm.artifact_kind, Some(ArtifactKind::Task));
    let is_implementation =
        matches!(fm.artifact_kind, Some(ArtifactKind::Implementation));
    let is_implementation_plan =
        matches!(fm.artifact_kind, Some(ArtifactKind::ImplementationPlan));
    // Cascade-root artifacts are the only places the full SDLC skill
    // toolbar (Run skill / Generate Cascade / Play) is shown:
    //   - master_requirement (project root, runs A0→A4 + Architecture)
    //   - task (runs only `07a-sde-plan-task` to produce an
    //     ImplementationPlan note; the user reviews the plan before
    //     pressing Play on the plan to execute it)
    // ImplementationPlan artifacts get a **Play button only**: it
    // kicks off the execute tail (`07b-sde-execute-implementation`
    // → `08-sde-generate-tests` → `09-sde-execute-tests`).
    // Implementation artifacts (the executed record) also get a Play
    // button that regenerates TestCases + reruns them without
    // redoing the code work, so a user who hand-edited the
    // Implementation body or the source can refresh tests in one
    // click.
    // Every other artifact kind — including a `Requirements` artifact
    // at the tree root, which used to be the entry point in the
    // legacy seed-skills-employee chain — hides the entire toolbar.
    // Only the Approve / Reject / Mark-dirty / Revise actions remain
    // on those kinds. Legacy projects can keep working by adding a
    // wrapping `master_requirement` artifact as their new project
    // root, or by running individual skills via the Workflow canvas.
    let is_cascade_root = is_master_requirement || is_task;
    let is_clarification =
        matches!(fm.artifact_kind, Some(ArtifactKind::Clarification));
    // Show the inline answer panel only while the clarification is
    // still awaiting an answer. Once Approved the parsed answer lives
    // in the body's `## Answer` section the writeback helper appended,
    // so the markdown render is enough — we hide the form to make it
    // obvious the question's been resolved.
    let show_clarification_panel =
        is_clarification && !matches!(status, ArtifactStatus::Approved);
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
    // Pick the primary Play/Stop run-mode this artifact deserves
    // (if any) given its kind + status. Drives the new header slot
    // that sits LEFT of the status pill — putting the run affordance
    // immediately next to the kind title so the user finds it
    // before the status. `None` hides the button; the right-side
    // actions toolbar (Run skill / Generate Cascade / etc.) is
    // unaffected. Centralising the mode-by-kind decision here keeps
    // the header rsx readable.
    let primary_play_mode: Option<crate::plugins::artifact::cascade::RunMode> = {
        use crate::plugins::artifact::cascade::RunMode;
        if !body_editable {
            None
        } else if is_cascade_root && is_runnable_source {
            Some(if is_task {
                RunMode::TaskPlanOnly
            } else {
                RunMode::Full
            })
        } else if is_implementation_plan && !is_cascade_root
            && matches!(status, ArtifactStatus::Approved | ArtifactStatus::Dirty)
        {
            Some(RunMode::PlanExecuteAndTest)
        } else if is_implementation && !is_cascade_root
            && matches!(status, ArtifactStatus::Approved | ArtifactStatus::Dirty)
        {
            Some(RunMode::ImplementationRetest)
        } else {
            None
        }
    };
    // TEMP DIAGNOSTIC: surface the body-editable computation as data
    // attributes so the user can confirm via DevTools whether
    // `props.edit` and `on_change.is_some()` are both true at runtime
    // (the artifact-can't-type bug). Remove once the root cause is
    // fixed.
    let dbg_edit = if props.edit { "true" } else { "false" };
    let dbg_on_change_some = if on_change.is_some() { "true" } else { "false" };
    let dbg_body_editable = if body_editable { "true" } else { "false" };
    rsx! {
        div { class: "operon-artifact-surface",
            "data-testid": "artifact-surface",
            "data-artifact-status": "{status.as_str()}",
            "data-artifact-kind": "{fm.artifact_kind.as_ref().map(|k| k.as_str().to_string()).unwrap_or_default()}",
            "data-debug-edit": "{dbg_edit}",
            "data-debug-on-change-some": "{dbg_on_change_some}",
            "data-debug-body-editable": "{dbg_body_editable}",
            div { class: "operon-artifact-header",
                span { class: "operon-artifact-kind-badge", "{kind_label}" }
                // Primary Play/Stop toggle, immediately next to the
                // kind title. CascadePlayButton morphs to ⏹ Stop on
                // its own when a cascade rooted on this artifact is
                // in flight, so one button covers both states.
                if let (Some(uuid), Some(mode)) = (source_uuid, primary_play_mode) {
                    CascadePlayButton {
                        root_artifact_id: uuid,
                        run_mode: mode,
                    }
                }
                span {
                    class: "operon-artifact-status-pill {status_class}",
                    "data-testid": "artifact-status-pill",
                    "{status_label}"
                }
                // Revision dropdown: only show when the body has stashed
                // prior revisions (i.e. the seed-skills re-run path
                // emitted `<details><summary>Revision N…</summary>`
                // blocks). Selecting Current returns the editable head
                // body; selecting a prior revision swaps the body to
                // read-only MarkdownView of that revision.
                if revisions.len() > 1 {
                    select {
                        class: "operon-artifact-revision-select",
                        "data-testid": "artifact-revision-select",
                        title: "View a prior revision of this artifact (read-only).",
                        value: match active_revision_idx {
                            None => "0".to_string(),
                            Some(i) => i.to_string(),
                        },
                        onchange: move |evt| {
                            match evt.value().parse::<usize>() {
                                Ok(0) => selected_revision.set(None),
                                Ok(i) if i < revisions.len() => {
                                    selected_revision.set(Some(i));
                                }
                                _ => selected_revision.set(None),
                            }
                        },
                        for (i, rev) in revisions.iter().enumerate() {
                            option {
                                value: "{i}",
                                "{rev.label}"
                            }
                        }
                    }
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
                            if let (Some(parent), Some(skill), Some(self_id)) =
                                (fm.source_artifact_id, fm.source_skill_id, source_uuid)
                            {
                                RerunButton {
                                    parent_id: parent,
                                    skill_id: skill,
                                    artifact_id: self_id,
                                    artifact_body: content_for_actions.clone(),
                                }
                            }
                        }
                        if is_cascade_root {
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
                                    GenerateCascadeButton { root_artifact_id: uuid }
                                }
                            }
                        }
                        // Secondary "Create test cases" button —
                        // regen-only (no rerun) variant of the
                        // Implementation Play. Lives in the action
                        // strip so the primary Play/Stop next to the
                        // title stays a single button. Dirty
                        // Implementations only.
                        if is_implementation
                            && !is_cascade_root
                            && matches!(status, ArtifactStatus::Dirty)
                        {
                            if let Some(uuid) = source_uuid {
                                CascadePlayButton {
                                    root_artifact_id: uuid,
                                    run_mode: crate::plugins::artifact::cascade::RunMode::GenerateTestCasesOnly,
                                }
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
            // Phase E: pending-review banner. Renders only when this
            // artifact is the Architecture AND it has `needs_review`
            // set in its frontmatter. Lists the titles of child
            // `architecture_review` notes so the user can spot them
            // in the explorer tree below.
            {
                let is_architecture = fm
                    .artifact_kind
                    .as_ref()
                    .map(|k| matches!(k, ArtifactKind::Architecture))
                    .unwrap_or(false);
                if is_architecture && fm.needs_review {
                    let review_titles: Vec<String> = source_uuid
                        .and_then(|sid| drop_note_repo.find_project_for_note(sid).ok().flatten().map(|p| (sid, p)))
                        .and_then(|(sid, pid)| {
                            drop_note_repo.list_for_project(pid).ok().map(|notes| {
                                notes
                                    .into_iter()
                                    .filter(|n| n.parent_id == Some(sid))
                                    .filter(|n| matches!(n.kind, NoteKind::Artifact))
                                    .map(|n| n.title)
                                    .collect()
                            })
                        })
                        .unwrap_or_default();
                    let count = review_titles.len();
                    let suffix = if count == 1 { "" } else { "s" };
                    rsx! {
                        div {
                            class: "operon-artifact-run-status operon-artifact-needs-review",
                            "data-testid": "artifact-needs-review-banner",
                            "\u{26A0} {count} pending architecture review{suffix} from later phase{suffix}. Open and approve or reject each one to clear the flag."
                            if !review_titles.is_empty() {
                                ul { class: "operon-artifact-needs-review-list",
                                    for t in review_titles.iter() {
                                        li { "{t}" }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    rsx! {}
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
                Some(crate::shell::companion_state::CascadePhase::Paused { level, .. }) => rsx! {
                    div {
                        class: "operon-artifact-run-status",
                        "data-testid": "artifact-cascade-status",
                        "Cascade paused at checkpoint (level {level}) \u{2014} review the new backlog and approve to continue."
                    }
                },
            }
            // The user-facing "Refinement notes" editor was removed.
            // The underlying `revision_notes` frontmatter field stays —
            // it's still set by the ClarificationPanel writeback and
            // still inlined into the regeneration prompt under
            // `--- refinement notes from user ---` by
            // `run_skill_on_source_with_revision_notes`. We just don't
            // expose a manual editor for it any more; refinement is
            // driven via clarifications + the markdown body itself.
            if show_clarification_panel {
                {
                    let parsed = parse_clarification(&body_only);
                    let note_repo_for_submit = drop_note_repo.clone();
                    let persistence_for_submit = persistence_for_clarification.clone();
                    let content_for_submit = content_for_actions.clone();
                    let on_change_for_submit = on_change;
                    let note_id_str_for_submit = props.note_id.clone();
                    let targets_for_submit = parsed.resolution_targets.clone();
                    rsx! {
                        ClarificationPanel {
                            clarification: parsed,
                            on_submit: EventHandler::new(move |answer: ClarificationAnswer| {
                                // 1. Rewrite the clarification's own body
                                //    with the user's answer appended and
                                //    status flipped to Approved. Pushed
                                //    through `on_change` so the standard
                                //    save / version-bump path runs.
                                let new_body = clarification_body_with_answer(
                                    &content_for_submit,
                                    &answer,
                                );
                                if let Some(handler) = on_change_for_submit {
                                    handler.call(new_body);
                                }
                                // 2. Best-effort writeback to each
                                //    `## Resolution target` artifact: append
                                //    the answer to `revision_notes` and mark
                                //    it Dirty so the next Play regenerates
                                //    it with the resolved direction. Runs
                                //    async so the form doesn't block on it;
                                //    failures log but don't surface (the
                                //    clarification's own state has already
                                //    been recorded).
                                let note_repo = note_repo_for_submit.clone();
                                let persistence = persistence_for_submit.clone();
                                let answer = answer.clone();
                                let targets = targets_for_submit.clone();
                                let id_str = note_id_str_for_submit.clone();
                                spawn(async move {
                                    let Ok(self_id) = Uuid::parse_str(&id_str) else {
                                        return;
                                    };
                                    let Ok(Some(project_id)) =
                                        note_repo.find_project_for_note(self_id)
                                    else {
                                        return;
                                    };
                                    let n = apply_clarification_answer_to_targets(
                                        &note_repo,
                                        &persistence,
                                        project_id,
                                        &answer,
                                        &targets,
                                    )
                                    .await;
                                    tracing::info!(
                                        target: "operon::clarification",
                                        "clarification {self_id}: wrote answer to {n} resolution target(s)"
                                    );
                                });
                            }),
                        }
                    }
                }
            }
            div {
                // In edit mode, the inner MonacoEditorHost mounts with
                // `position: absolute; inset: 0;`, which only works when
                // its parent is positioned. The `-edit` modifier also
                // drops the body padding so Monaco fills the surface
                // edge to edge — preview mode keeps the padded layout.
                // When viewing a prior revision from the dropdown we
                // never mount Monaco — the historical body is render-
                // only — so we always pick the padded preview class.
                class: if body_editable && !viewing_prior_revision {
                    "operon-artifact-body operon-artifact-body-edit"
                } else {
                    "operon-artifact-body"
                },
                if let Some(rev_body) = prior_revision_body.clone() {
                    MarkdownView { content: rev_body }
                } else if body_editable {
                    {
                        // Monaco edits the BODY only; we re-attach the
                        // (untouched) frontmatter before pushing the
                        // change up so the status pill / source linkage
                        // survive. Mirrors what the previous <textarea>
                        // did, but routes through MonacoEditorHost so
                        // the artifact editor matches every other
                        // markdown surface (Cmd+K link picker, paste-
                        // image, drag-drop). The captured `fm_for_monaco`
                        // is the parsed frontmatter at the start of this
                        // render — re-derived next render once
                        // props.content updates.
                        let on_body_change = {
                            let content_for_monaco = content_for_monaco.clone();
                            let fm_for_monaco = fm_for_monaco.clone();
                            EventHandler::new(move |new_body: String| {
                                // Auto-mark Dirty when an Approved
                                // ImplementationPlan / Implementation
                                // body changes. For a plan, this
                                // flips the Play button so the next
                                // click re-executes against the
                                // edited plan; for an executed
                                // Implementation, it surfaces the
                                // "Create test cases" button + keeps
                                // the retest Play active. Restricted to
                                // those kinds + Approved so editing a
                                // Pending / Rejected artifact (i.e.
                                // the user is still in initial review)
                                // doesn't silently mark it Dirty.
                                let mut fm_for_save = fm_for_monaco.clone();
                                let auto_dirty_kind = matches!(
                                    fm_for_save.artifact_kind,
                                    Some(ArtifactKind::Implementation)
                                        | Some(ArtifactKind::ImplementationPlan)
                                );
                                if auto_dirty_kind
                                    && fm_for_save.status == ArtifactStatus::Approved
                                {
                                    fm_for_save.status = ArtifactStatus::Dirty;
                                }
                                let recombined = rewrite(&content_for_monaco, &fm_for_save);
                                let final_doc = replace_body(&recombined, &new_body);
                                if let Some(handler) = on_change {
                                    handler.call(final_doc);
                                }
                            })
                        };
                        let on_keyaction = EventHandler::new(move |action: String| {
                            // The Monaco bootstrap intercepts Cmd+K and
                            // Cmd+Shift+I and posts these action names. We
                            // open the corresponding picker; its on_pick
                            // splices the formatted markdown link back
                            // into Monaco via `monaco_channel`.
                            match action.as_str() {
                                "linkpicker" => link_picker_open.set(true),
                                "imagepicker" => image_picker_open.set(true),
                                _ => {}
                            }
                        });
                        rsx! {
                            div {
                                // Outer drop target: in-app note drags
                                // from the explorer (DragSession) are
                                // converted to a markdown link and
                                // spliced into Monaco. Sized to fill
                                // the artifact-body so a drop anywhere
                                // over the editor lands here. Monaco
                                // mounts inside via MonacoEditorHost's
                                // own absolute-inset wrapper.
                                style: "position: absolute; inset: 0;",
                                ondragover: move |evt| evt.prevent_default(),
                                ondrop: {
                                    let note_repo = drop_note_repo.clone();
                                    let project_repo = drop_project_repo.clone();
                                    let mut drag_session = drag_session;
                                    move |evt: Event<DragData>| {
                                        let kind = *drag_session.peek();
                                        let note_id = match kind {
                                            Some(crate::local_mode::ui::DragKind::Note(id)) => id,
                                            _ => return,
                                        };
                                        evt.prevent_default();
                                        // Resolve the note's title +
                                        // kind across all projects.
                                        let projects = project_repo.list().unwrap_or_default();
                                        let mut found: Option<(String, operon_store::repos::NoteKind)> = None;
                                        for p in &projects {
                                            if let Ok(notes) = note_repo.list_for_project(p.id) {
                                                if let Some(n) =
                                                    notes.into_iter().find(|n| n.id == note_id)
                                                {
                                                    found = Some((n.title, n.kind));
                                                    break;
                                                }
                                            }
                                        }
                                        let Some((title, kind)) = found else {
                                            drag_session.set(None);
                                            return;
                                        };
                                        let inserted = if matches!(
                                            kind,
                                            operon_store::repos::NoteKind::Image
                                        ) {
                                            format!(
                                                "![{}](operon://note/{})",
                                                title, note_id
                                            )
                                        } else {
                                            format!(
                                                "[{}](operon://note/{})",
                                                title, note_id
                                            )
                                        };
                                        if let Some(channel) = monaco_channel.peek().as_ref().cloned() {
                                            channel.splice(&inserted);
                                        }
                                        drag_session.set(None);
                                    }
                                },
                                MonacoEditorHost {
                                    note_id: props.note_id.clone(),
                                    content: monaco_body.clone(),
                                    language: LanguageDescriptor::markdown(),
                                    on_change: on_body_change,
                                    channel_sink: monaco_channel,
                                    on_action: on_keyaction,
                                }
                            }
                            ArtifactPickerMounts {
                                link_open: link_picker_open,
                                image_open: image_picker_open,
                                channel: monaco_channel,
                            }
                        }
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
    let new_body = patch_status_text(content, next);
    if let Some(handler) = on_change {
        handler.call(new_body);
    }
}

/// Persist a status flip immediately and notify the open editor tab.
/// Saves bypass the SaveScheduler so the new state sticks in
/// View-mode (no `on_change`) and in Edit-mode manual-save tabs (the
/// scheduler short-circuits there). The `on_change` notification
/// keeps the in-memory tab content in sync so the visible body
/// reflects the patched frontmatter without a reload.
///
/// `note_id` is the artifact's UUID-string (the persistence key);
/// `content` is the current body. The save is fire-and-forget via
/// `dioxus::prelude::spawn` — failures are logged at warn and
/// silently ignored (matches the rest of the artifact view's
/// best-effort save calls).
fn save_status_change(
    persistence: &Arc<dyn Persistence>,
    note_id: &str,
    content: &str,
    next: ArtifactStatus,
    on_change: Option<EventHandler<String>>,
) {
    let new_body = patch_status_text(content, next);
    if let Some(handler) = on_change {
        handler.call(new_body.clone());
    }
    let persistence = persistence.clone();
    let note_id_str = note_id.to_string();
    let note_id_for_clear = note_id_str.clone();
    let body = new_body;
    let prev_fm = parse(content);
    let next_fm = parse(&body);
    let note_repo_for_clear: Option<Arc<dyn LocalNoteRepository>> =
        try_consume_context::<LocalNoteRepo>().map(|c| c.0);
    dioxus::prelude::spawn(async move {
        if let Err(e) = persistence.save(&note_id_str, body.as_bytes()).await {
            tracing::warn!(
                target: "operon::artifact",
                "save_status_change: persistence.save({note_id_str}) failed: {e}"
            );
            return;
        }
        // Phase E auto-clear: if the note we just stamped was an
        // `architecture_review` whose status moved from a Pending/
        // Dirty state to Approved/Rejected, re-scan the parent
        // architecture's review children. When none remain
        // Pending/Dirty, clear the architecture's `needs_review`
        // flag so the explorer / canvas badges drop.
        let became_resolved = matches!(
            prev_fm.status,
            ArtifactStatus::Pending | ArtifactStatus::Dirty
        ) && matches!(
            next_fm.status,
            ArtifactStatus::Approved | ArtifactStatus::Rejected
        );
        let is_review = next_fm
            .artifact_kind
            .as_ref()
            .map(|k| matches!(k, ArtifactKind::ArchitectureReview))
            .unwrap_or(false);
        if !(became_resolved && is_review) {
            return;
        }
        let Some(note_repo) = note_repo_for_clear else { return };
        let Ok(this_id) = Uuid::parse_str(&note_id_for_clear) else { return };
        // Find parent architecture id.
        let Ok(Some(project_id)) = note_repo.find_project_for_note(this_id) else {
            return;
        };
        let Ok(notes) = note_repo.list_for_project(project_id) else { return };
        let parent_id = match notes.iter().find(|n| n.id == this_id) {
            Some(n) => n.parent_id,
            None => return,
        };
        let Some(parent_id) = parent_id else { return };
        // Any remaining Pending/Dirty review siblings keep the flag.
        let mut any_pending = false;
        for sibling in notes.iter().filter(|n| n.parent_id == Some(parent_id)) {
            if sibling.id == this_id {
                continue;
            }
            if !matches!(sibling.kind, NoteKind::Artifact) {
                continue;
            }
            let bytes = match persistence.load(&sibling.id.to_string()).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let body = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let fm = parse(&body);
            let is_review_kind = fm
                .artifact_kind
                .as_ref()
                .map(|k| matches!(k, ArtifactKind::ArchitectureReview))
                .unwrap_or(false);
            if is_review_kind
                && matches!(fm.status, ArtifactStatus::Pending | ArtifactStatus::Dirty)
            {
                any_pending = true;
                break;
            }
        }
        if !any_pending {
            crate::plugins::artifact::runner::flip_needs_review_on(
                &persistence,
                parent_id,
                false,
            )
            .await;
            // Bump LocalNoteVersion so the explorer + canvas badges
            // refresh on the next render tick.
            *crate::shell::companion_state::LOCAL_NOTE_VERSION.write() += 1;
        }
    });
}

/// Pure-text variant of `patch_status` — returns the rewritten body
/// instead of dispatching it through an `EventHandler`. The workflow
/// plugin uses this when it needs to write the artifact frontmatter
/// directly via `Persistence::save` (the artifact note isn't open
/// in a tab when the user clicks Approve / Reject / Mark dirty on
/// the workflow card).
pub fn patch_status_text(content: &str, next: ArtifactStatus) -> String {
    let mut fm = parse(content);
    fm.status = next;
    rewrite(content, &fm)
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

    #[test]
    fn iso_date_from_unix_secs_handles_epoch_and_known_dates() {
        // Epoch.
        assert_eq!(iso_date_from_unix_secs(0), "1970-01-01");
        // 2026-05-11 00:00:00 UTC = 1778457600
        assert_eq!(iso_date_from_unix_secs(1_778_457_600), "2026-05-11");
        // Leap-day check: 2024-02-29 00:00:00 UTC = 1709164800
        assert_eq!(iso_date_from_unix_secs(1_709_164_800), "2024-02-29");
        // Last second of 2023-12-31 → still 2023-12-31.
        assert_eq!(iso_date_from_unix_secs(1_704_067_199), "2023-12-31");
        // First second of 2024-01-01 → 2024-01-01.
        assert_eq!(iso_date_from_unix_secs(1_704_067_200), "2024-01-01");
    }

    #[test]
    fn format_clarification_answer_block_renders_selected_and_other() {
        let answer = ClarificationAnswer {
            selected: vec!["Keep scope".into(), "Add SLO".into()],
            other: "freeze marketing".into(),
        };
        let block = format_clarification_answer_block(&answer);
        assert!(block.starts_with("\n## Answer ("));
        assert!(block.contains("- Keep scope"));
        assert!(block.contains("- Add SLO"));
        assert!(block.contains("- Other: freeze marketing"));
    }

    #[test]
    fn format_clarification_answer_block_handles_empty_selection() {
        let answer = ClarificationAnswer::default();
        let block = format_clarification_answer_block(&answer);
        assert!(block.contains("_(no option selected)_"));
    }

    #[test]
    fn clarification_body_with_answer_appends_and_approves() {
        let body = "---\n\
            artifact_kind: clarification\n\
            status: pending\n\
            ---\n\
            \n\
            # Clarification: scope\n\
            \n\
            ## Options\n\
            - [ ] Keep\n\
            - [ ] Drop\n";
        let answer = ClarificationAnswer {
            selected: vec!["Keep".into()],
            other: String::new(),
        };
        let next = clarification_body_with_answer(body, &answer);
        assert!(next.contains("status: approved"));
        assert!(next.contains("# Clarification: scope"));
        assert!(next.contains("## Answer ("));
        assert!(next.contains("- Keep"));
    }

    #[test]
    fn summarize_answer_for_notes_joins_selected_and_other() {
        let answer = ClarificationAnswer {
            selected: vec!["Keep".into(), "Defer".into()],
            other: "split into v2".into(),
        };
        assert_eq!(
            summarize_answer_for_notes(&answer),
            "Keep; Defer; Other: split into v2"
        );
    }

    #[test]
    fn summarize_answer_for_notes_returns_empty_for_empty_answer() {
        assert_eq!(summarize_answer_for_notes(&ClarificationAnswer::default()), "");
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
                                                None,
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
    extra_revision_notes: Option<(Uuid, String)>,
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
        // Single-skill Re-run from the artifact view's "Re-run" /
        // "Run skill" buttons. No Stop UI here yet, so pass a fresh
        // CancellationToken — the runner's signature requires one.
        // Once a per-artifact Stop button lands, wire it through.
        let cancel = tokio_util::sync::CancellationToken::new();
        let result =
            crate::plugins::artifact::runner::run_skill_on_source_with_revision_notes(
                &note_repo,
                &project_repo,
                &persistence,
                &plugin,
                Some(&chat_message_repo),
                chat_session_id,
                source_note_id,
                skill_note_id,
                extra_revision_notes,
                Vec::new(),
                cancel,
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
    /// The Dirty output artifact whose Re-run button this is. Used to
    /// pull the user's `revision_notes` off its frontmatter so they
    /// can be inlined into the regeneration prompt under
    /// `--- refinement notes from user ---`. The runner clears the
    /// notes after a successful run.
    artifact_id: Uuid,
    /// The current rendered body of the Dirty artifact (already in
    /// memory in `ArtifactView`). Pre-flushed to disk by `spawn_runner`
    /// before the run starts so the parsed `revision_notes` reflects
    /// the user's latest edit, not stale persistence bytes.
    artifact_body: String,
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
    let artifact_id = props.artifact_id;
    let artifact_body = props.artifact_body.clone();

    rsx! {
        button {
            r#type: "button",
            class: "operon-artifact-rerun",
            "data-testid": "artifact-rerun",
            title: "Re-run the producing skill against the parent — overwrites this artifact. If you've added Refinement notes, they're inlined into the regeneration prompt.",
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
                // Pull the user's refinement notes off this dirty
                // artifact's frontmatter so they reach the regenerator
                // under the `--- refinement notes from user ---`
                // prompt fence. None when the user didn't type any
                // notes — the runner falls back to source-side notes
                // and/or no fence at all.
                let extra_notes = crate::plugins::artifact::frontmatter::parse(&artifact_body)
                    .revision_notes
                    .map(|n| (artifact_id, n));
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
                    extra_notes,
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

/// Render the user's answer to a clarification artifact into a
/// markdown block suitable for appending to the artifact's body.
/// Carries the selected option labels, any free-text "Other" input,
/// and an ISO date so the audit trail survives in the note.
pub fn format_clarification_answer_block(answer: &ClarificationAnswer) -> String {
    use std::fmt::Write as _;
    let today = current_iso_date();
    let mut out = String::new();
    writeln!(out, "\n## Answer ({today})").ok();
    if !answer.selected.is_empty() {
        for label in &answer.selected {
            writeln!(out, "- {label}").ok();
        }
    }
    if !answer.other.is_empty() {
        writeln!(out, "- Other: {}", answer.other).ok();
    }
    if answer.selected.is_empty() && answer.other.is_empty() {
        writeln!(out, "- _(no option selected)_").ok();
    }
    out
}

/// `YYYY-MM-DD` today in UTC. Done by hand rather than via `chrono`
/// because the dependency isn't already in `operon-dioxus` and this
/// is the only date-format consumer for now. Also reused by the
/// typed-artifact scaffold builder in
/// `src/local_mode/explorer/creatable_kind.rs` to stamp the seed
/// `## Revision history` row.
pub(crate) fn current_iso_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_date_from_unix_secs(secs)
}

/// Pure helper for `current_iso_date` — lets the date logic be tested
/// without freezing the clock. Implements the proleptic Gregorian
/// calendar math directly (no chrono).
fn iso_date_from_unix_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Days from 1970-01-01 (epoch) to 0000-03-01 (proleptic Gregorian,
    // year-month-day computation reference). 1970-01-01 is day 719468
    // counting from 0000-03-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Compute the new clarification body: append the answer block at the
/// end and flip the frontmatter to Approved. Used by both the inline
/// submit handler and tests.
pub fn clarification_body_with_answer(
    content: &str,
    answer: &ClarificationAnswer,
) -> String {
    let answer_block = format_clarification_answer_block(answer);
    let mut fm = parse(content);
    fm.status = ArtifactStatus::Approved;
    let rewritten = rewrite(content, &fm);
    // The seed-skill convention is to keep the question prose at the
    // top of the body; the answer block lands at the bottom so the
    // markdown view shows the question first, then the resolution.
    if rewritten.ends_with('\n') {
        format!("{rewritten}{answer_block}")
    } else {
        format!("{rewritten}\n{answer_block}")
    }
}

/// Best-effort writeback: for each resolution-target slug, find a
/// sibling artifact note in the same project whose title matches the
/// slug, append the user's answer to its `revision_notes`, and flip
/// it to Dirty so the next cascade Play regenerates it with the
/// resolved direction. Returns the count of artifacts mutated.
///
/// Slug matching strips a leading `epic-`/`feature-`/etc. prefix is
/// NOT necessary — the seed-skill convention names artifacts by
/// their full slug (e.g. `epic-02-billing`) and the corresponding
/// note title is the same string. Falls back to exact title match
/// against every note in the project.
pub async fn apply_clarification_answer_to_targets(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    answer: &ClarificationAnswer,
    targets: &[String],
) -> usize {
    if targets.is_empty() {
        return 0;
    }
    let summary = summarize_answer_for_notes(answer);
    if summary.is_empty() {
        return 0;
    }
    let all = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "operon::clarification",
                "list_for_project for writeback failed: {e}"
            );
            return 0;
        }
    };
    let by_title: HashMap<&str, &LocalNote> = all
        .iter()
        .filter(|n| matches!(n.kind, NoteKind::Artifact))
        .map(|n| (n.title.as_str(), n))
        .collect();
    let mut changed = 0usize;
    for target in targets {
        let key = target.trim().trim_matches('`');
        let Some(note) = by_title.get(key) else {
            tracing::debug!(
                target: "operon::clarification",
                "writeback skipped: no artifact in project {project_id} with title {key:?}"
            );
            continue;
        };
        let bytes = match persistence.load(&note.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut fm = parse(&body);
        let merged = match fm.revision_notes.as_deref() {
            Some(existing) if !existing.trim().is_empty() => {
                format!("{existing}\n\n[from clarification]: {summary}")
            }
            _ => format!("[from clarification]: {summary}"),
        };
        fm.revision_notes = Some(merged);
        // Mark dirty so the dirty-regen cascade picks it up on the
        // next Play. Skip the flip when the artifact is already
        // Rejected — the user explicitly said no to that line of
        // work and a clarification answer shouldn't override that
        // signal.
        if !matches!(fm.status, ArtifactStatus::Rejected) {
            fm.status = ArtifactStatus::Dirty;
        }
        let new_body = rewrite(&body, &fm);
        if persistence
            .save(&note.id.to_string(), new_body.as_bytes())
            .await
            .is_ok()
        {
            changed += 1;
        }
    }
    changed
}

/// Flatten a `ClarificationAnswer` into a one-line string suitable
/// for embedding inside another artifact's `revision_notes`. Selected
/// options are comma-joined; an "Other" value (if any) is appended
/// after a semicolon.
fn summarize_answer_for_notes(answer: &ClarificationAnswer) -> String {
    let mut parts: Vec<String> = answer.selected.clone();
    let other = answer.other.trim();
    if !other.is_empty() {
        parts.push(format!("Other: {other}"));
    }
    parts.join("; ")
}

/// Walk every Artifact descendant of `root_id` in the note tree and
/// flip its status from Approved → Dirty. Returns the number of rows
/// mutated. Errors are best-effort: a single load/save failure
/// doesn't abort the walk.
pub async fn mark_descendants_dirty(
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
    /// Which slice of the SDLC chain this Play click should run. The
    /// parent (`ArtifactView`) picks the mode from the artifact's kind
    /// + status:
    ///   - Task → `TaskPlanOnly` (07a only — produces the
    ///     ImplementationPlan note, stops for review)
    ///   - ImplementationPlan + Approved/Dirty → `PlanExecuteAndTest`
    ///     (07b code + commit, 08 tests, 09 run)
    ///   - Implementation + Approved/Dirty → `ImplementationRetest`
    ///     (08 + 09: regen tests against current code + rerun)
    ///   - Master / legacy → `Full`
    /// `GenerateTestCasesOnly` (08 only) is dispatched by the separate
    /// "Create test cases" button on Dirty Implementations, not this
    /// Play one.
    #[props(default)]
    run_mode: crate::plugins::artifact::cascade::RunMode,
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

    // Pre-flight dependency check. Walk up to the seed, ask
    // `unmet_dep_titles` for prerequisites that aren't yet `Approved`.
    // Reading `LocalNoteVersion` inside the async block subscribes the
    // resource to it, so an Approve / Reject elsewhere bumps that
    // signal and re-runs us with fresh status.
    let unmet_deps_resource = {
        let note_repo = note_repo.clone();
        let persistence = persistence.clone();
        let mut note_version = note_version;
        use_resource(move || {
            let note_repo = note_repo.clone();
            let persistence = persistence.clone();
            async move {
                // Subscribe to LocalNoteVersion so approve/reject elsewhere
                // re-fires this resource. The read value itself doesn't
                // matter — we just want the dependency wired up.
                let _v = *note_version.read();
                let project_id = match note_repo.find_project_for_note(root_id) {
                    Ok(Some(p)) => p,
                    _ => return Vec::new(),
                };
                let seed_id = resolve_seed_id_sync(&persistence, root_id);
                crate::plugins::artifact::cascade::unmet_dep_titles(
                    &note_repo,
                    &persistence,
                    project_id,
                    seed_id,
                    root_id,
                )
                .await
            }
        })
    };
    let unmet_titles: Vec<String> = unmet_deps_resource
        .read()
        .as_ref()
        .cloned()
        .unwrap_or_default();
    let blocked = !unmet_titles.is_empty();
    let blocked_tooltip = if blocked {
        format!(
            "Approve these first: {}",
            unmet_titles.join(", ")
        )
    } else {
        String::new()
    };

    // Label / testid / class vary by run mode so a single component
    // serves the ▶ Play button on most artifacts AND the
    // "Create test cases" button on a Dirty Implementation. Other
    // run-mode behaviours (which skills fire) are handled inside
    // `run_cascade`'s `filter_skills_for_run_mode` step.
    let is_test_cases_button = matches!(
        props.run_mode,
        crate::plugins::artifact::cascade::RunMode::GenerateTestCasesOnly
    );
    let (label_play, label_stop, idle_class, idle_title, testid_attr) =
        if is_test_cases_button {
            (
                "Create test cases",
                "\u{23F9} Stop",
                "operon-artifact-cascade-generate-tests",
                "Regenerate test cases against the current Implementation body."
                    .to_string(),
                "artifact-generate-test-cases",
            )
        } else {
            (
                "\u{25B6} Play",
                "\u{23F9} Stop",
                "operon-artifact-cascade-play",
                "Run the SDLC pipeline from this artifact \u{2014} every produced child auto-approves."
                    .to_string(),
                "artifact-cascade-play",
            )
        };
    rsx! {
        button {
            r#type: "button",
            class: if is_running { "operon-artifact-cascade-stop" } else { idle_class },
            "data-testid": "{testid_attr}",
            disabled: blocked && !is_running,
            title: if is_running {
                "Stop the cascade at the next skill boundary.".to_string()
            } else if blocked {
                blocked_tooltip.clone()
            } else {
                idle_title.clone()
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
                        // Belt-and-suspenders: even if the resource is
                        // mid-resolve and the disabled attr hasn't taken
                        // effect, refuse to launch a doomed cascade run.
                        if blocked {
                            return;
                        }
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
                            None, // full cascade — no depth cap
                            props.run_mode,
                        );
                    }
                }
            },
            if is_running { "{label_stop}" } else { "{label_play}" }
        }
        // Stage-picker chevron only makes sense for the generic Play
        // button — the "Create test cases" variant runs a fixed
        // single-skill mode (08), so exposing a stage picker would
        // confuse users into thinking they can toggle stages off.
        if !is_test_cases_button {
            button {
                r#type: "button",
                class: "operon-artifact-cascade-stages-toggle",
                "data-testid": "artifact-cascade-stages-toggle",
                title: "Configure which pipeline stages run when you click Play.",
                onclick: move |_| stages_open.with_mut(|v| *v = !*v),
                "\u{25BE}"
            }
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

/// Format a `PickedLink` as `[Title](operon://note/<uuid>)` for note
/// hits, or as the bare title for project hits (which don't have a
/// note id). Used by the artifact link-picker `on_pick` handler.
#[cfg(not(target_arch = "wasm32"))]
fn format_picked_as_markdown_link(picked: &crate::local_mode::editor::PickedLink) -> String {
    match picked.note_id {
        Some(id) => format!("[{}](operon://note/{})", picked.title, id),
        None => picked.title.clone(),
    }
}

/// Image-embed variant of `format_picked_as_markdown_link`: always
/// emits `![alt](operon://note/<uuid>)` so the markdown renderer's
/// image-kind resolver renders an inline `<img>` (or a card preview
/// for non-image notes — matches the existing Cmd+Shift+I behaviour
/// in `LocalNoteEditor`).
#[cfg(not(target_arch = "wasm32"))]
fn format_picked_as_markdown_image(picked: &crate::local_mode::editor::PickedLink) -> String {
    match picked.note_id {
        Some(id) => format!("![{}](operon://note/{})", picked.title, id),
        None => picked.title.clone(),
    }
}

/// Cfg-gated wrapper around `LinkPicker` so the artifact view compiles
/// on wasm (where the picker isn't built). Desktop: when a picker
/// signal is open, mounts `LinkPicker` and splices the formatted
/// markdown link back into Monaco via `channel`. Wasm: no-op.
#[derive(Props, Clone, PartialEq)]
struct ArtifactPickerMountsProps {
    link_open: Signal<bool>,
    image_open: Signal<bool>,
    channel: Signal<Option<MonacoChannel>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[component]
fn ArtifactPickerMounts(props: ArtifactPickerMountsProps) -> Element {
    let mut link_open = props.link_open;
    let mut image_open = props.image_open;
    let channel = props.channel;

    // Plain-closure pattern (matches `LocalNoteEditor::on_pick_link`).
    // `EventHandler<PickedLink>` auto-coerces from a `FnMut + 'static`
    // closure; `Callback::new(...)` returns a different type whose
    // bridge through the EventHandler prop drops the call in some
    // contexts, so we avoid it here.
    let on_pick_link = move |picked: crate::local_mode::editor::PickedLink| {
        let inserted = format_picked_as_markdown_link(&picked);
        let snap = channel.peek().as_ref().cloned();
        eprintln!(
            "operon: artifact link-picker on_pick fired \u{2014} channel_some={} inserted={:?}",
            snap.is_some(),
            &inserted
        );
        if let Some(c) = snap {
            c.splice(&inserted);
        } else {
            eprintln!(
                "operon: artifact link-picker SKIPPED splice \u{2014} monaco_channel is None"
            );
        }
        link_open.set(false);
    };
    let on_pick_image = move |picked: crate::local_mode::editor::PickedLink| {
        let inserted = format_picked_as_markdown_image(&picked);
        let snap = channel.peek().as_ref().cloned();
        eprintln!(
            "operon: artifact image-picker on_pick fired \u{2014} channel_some={} inserted={:?}",
            snap.is_some(),
            &inserted
        );
        if let Some(c) = snap {
            c.splice(&inserted);
        } else {
            eprintln!(
                "operon: artifact image-picker SKIPPED splice \u{2014} monaco_channel is None"
            );
        }
        image_open.set(false);
    };

    rsx! {
        if *link_open.read() {
            crate::local_mode::editor::LinkPicker {
                open: link_open,
                on_pick: on_pick_link,
            }
        }
        if *image_open.read() {
            crate::local_mode::editor::LinkPicker {
                open: image_open,
                on_pick: on_pick_image,
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[component]
fn ArtifactPickerMounts(_props: ArtifactPickerMountsProps) -> Element {
    rsx! {}
}

/// Labeled "Generate Cascade" button rendered only on Requirements
/// artifacts (the user-facing root of the SDLC pipeline). Clicking it
/// opens (creating on first use) the `Cascade: <root title>` workflow
/// note for this Requirements artifact in a new tab. It does NOT spawn
/// the cascade orchestrator — running is the job of the neighboring
/// ▶ Play button (`CascadePlayButton`). This split keeps "show me the
/// cascade graph" cheap and side-effect-free; "actually run the
/// pipeline" stays explicit on Play.
#[derive(Props, Clone, PartialEq)]
struct GenerateCascadeButtonProps {
    root_artifact_id: Uuid,
}

#[component]
fn GenerateCascadeButton(props: GenerateCascadeButtonProps) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let persistence: Arc<dyn Persistence> = use_context();
    let tabs: Signal<crate::tabs::TabManager> = use_context();
    let save_scheduler: crate::tabs::SaveScheduler = use_context();
    let LocalNoteVersion(note_version) = use_context();

    let root_id = props.root_artifact_id;

    rsx! {
        button {
            r#type: "button",
            class: "operon-artifact-generate-cascade",
            "data-testid": "artifact-generate-cascade",
            title: "Open the cascade workflow for this Requirements note (creates one on first click). Use \u{25B6} Play to actually run the pipeline.",
            onclick: {
                let note_repo = note_repo.clone();
                let persistence = persistence.clone();
                let mut note_version_setter = note_version;
                move |_| {
                    open_cascade_workflow_tab(
                        root_id,
                        &note_repo,
                        &persistence,
                        tabs,
                        save_scheduler.clone(),
                        &mut note_version_setter,
                    );
                }
            },
            "Generate Cascade"
        }
    }
}

/// Resolve (or create) the `Cascade: <root>` workflow note for this
/// Requirements artifact and open it as an Edit tab. Pure navigation:
/// no orchestrator spawn, no chat-session bind, no `CASCADE_STATE`
/// mutation. Mirrors the explorer's note-click flow — the same
/// `open_local_note_tab` helper handles tab reuse + plugin dispatch.
fn open_cascade_workflow_tab(
    root_id: Uuid,
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    tabs: Signal<crate::tabs::TabManager>,
    save_scheduler: crate::tabs::SaveScheduler,
    note_version: &mut Signal<u64>,
) {
    let project_id = match note_repo.find_project_for_note(root_id) {
        Ok(Some(p)) => p,
        Ok(None) => {
            eprintln!(
                "operon: generate-cascade open: root {root_id} has no project"
            );
            return;
        }
        Err(e) => {
            eprintln!("operon: generate-cascade open: find_project_for_note: {e}");
            return;
        }
    };
    // Walk up the source_artifact_id chain to find the SEED so that
    // even if the GenerateCascadeButton ever gets mounted on a
    // non-Requirements artifact (today it's gated to Requirements),
    // we still reuse the seed's `Cascade: <seed title>` workflow
    // note instead of minting a new one. Idempotent for Requirements
    // because root_id == seed_id there.
    let seed_id = resolve_seed_id_sync(persistence, root_id);
    let root_title = note_repo
        .list_for_project(project_id)
        .ok()
        .and_then(|all| all.into_iter().find(|n| n.id == seed_id))
        .map(|n| n.title)
        .unwrap_or_else(|| short_uuid(seed_id));
    let (graph_note_id, was_created) =
        match crate::plugins::artifact::cascade_graph::ensure_cascade_workflow_note(
            note_repo,
            project_id,
            &root_title,
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "operon: generate-cascade open: ensure_cascade_workflow_note: {e}"
                );
                return;
            }
        };
    // Seed the freshly-created workflow with just the source-artifact
    // root snapshot. No placeholder kinds, no skill nodes — Play
    // populates real artifact-snapshot nodes via the cascade runner's
    // existing CascadeGraphWriter when the user actually runs.
    //
    // Bump LocalNoteVersion when the workflow note was freshly created
    // so the explorer's per-project note cache re-fetches and the new
    // `Cascade: <root>` row shows up in the project tree. The seed's
    // kind is Requirements (the GenerateCascadeButton is mounted only
    // on Requirements artifacts; if that gate ever loosens, the
    // resolve_seed_id_sync walk above still keeps us pointed at the
    // Requirements seed).
    if was_created {
        if let Err(e) = futures::executor::block_on(
            crate::plugins::artifact::cascade_graph::seed_cascade_workflow_root_only(
                persistence,
                graph_note_id,
                seed_id,
                "Requirements",
                &root_title,
            ),
        ) {
            eprintln!(
                "operon: generate-cascade open: seed_cascade_workflow_root_only: {e}"
            );
        }
        note_version.with_mut(|v| *v = v.saturating_add(1));
    }
    let tab_title = format!("Cascade: {}", root_title);
    let initial_content = {
        let id_str = graph_note_id.to_string();
        match futures::executor::block_on(persistence.load(&id_str)) {
            Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
            Err(crate::persistence::PersistError::NotFound) => String::new(),
            Err(e) => {
                eprintln!(
                    "operon: generate-cascade open: load {id_str}: {e:?}"
                );
                String::new()
            }
        }
    };
    crate::local_mode::editor::open_local_note_tab(
        tabs,
        save_scheduler,
        graph_note_id,
        tab_title,
        initial_content,
        operon_store::repos::NoteKind::Workflow,
    );
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

/// Walk up an artifact's `source_artifact_id` chain via persistence
/// loads + frontmatter parses until we hit a root (an artifact with
/// no `source_artifact_id` set). That topmost ancestor is the
/// "seed" — for SDLC cascades, typically the user's Requirements
/// note. Used by `spawn_cascade` to key the workflow note + chat
/// session, and by `CascadePlayButton` to scope the dependency
/// pre-flight check.
///
/// Synchronous via `block_on` because the callers run in Dioxus
/// click handlers / render bodies. Self-loop guard returns whatever
/// node we're at if a cycle is detected (paranoia — frontmatter
/// shouldn't contain cycles).
pub fn resolve_seed_id_sync(
    persistence: &Arc<dyn Persistence>,
    artifact_id: Uuid,
) -> Uuid {
    let persistence = persistence.clone();
    futures::executor::block_on(async move {
        let mut current = artifact_id;
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current) {
                break;
            }
            let body = match persistence.load(&current.to_string()).await {
                Ok(b) => match String::from_utf8(b) {
                    Ok(s) => s,
                    Err(_) => break,
                },
                Err(_) => break,
            };
            let fm = crate::plugins::artifact::frontmatter::parse(&body);
            match fm.source_artifact_id {
                Some(parent) => current = parent,
                None => break,
            }
        }
        current
    })
}

/// Load an artifact's body and pull its `artifact_kind` out of the
/// frontmatter. Returns `None` if the persistence load fails or the
/// frontmatter lacks an `artifact_kind` field. Sync via `block_on` for
/// the same reason as `resolve_seed_id_sync`.
fn resolve_artifact_kind_sync(
    persistence: &Arc<dyn Persistence>,
    artifact_id: Uuid,
) -> Option<crate::plugins::artifact::frontmatter::ArtifactKind> {
    let persistence = persistence.clone();
    futures::executor::block_on(async move {
        let body = persistence.load(&artifact_id.to_string()).await.ok()?;
        let s = String::from_utf8(body).ok()?;
        crate::plugins::artifact::frontmatter::parse(&s).artifact_kind
    })
}

/// Short bracketed tag rendered into the chat-session rail title so two
/// cascades on different artifacts under the same seed look different
/// (otherwise everything reads `Cascade: <seed>` and collides).
fn artifact_kind_tag(kind: &crate::plugins::artifact::frontmatter::ArtifactKind) -> &'static str {
    use crate::plugins::artifact::frontmatter::ArtifactKind;
    match kind {
        ArtifactKind::MasterRequirement => "MR",
        ArtifactKind::Requirements => "req",
        ArtifactKind::Epic => "epic",
        ArtifactKind::Feature => "feat",
        ArtifactKind::Story => "story",
        ArtifactKind::Task => "task",
        ArtifactKind::Plan => "plan",
        ArtifactKind::ImplementationPlan => "implplan",
        ArtifactKind::Implementation => "imp",
        ArtifactKind::TestCases => "tests",
        ArtifactKind::TestResults => "tres",
        ArtifactKind::Summary => "summary",
        ArtifactKind::Architecture => "arch",
        ArtifactKind::ArchitectureReview => "review",
        ArtifactKind::Bug => "bug",
        ArtifactKind::Clarification => "clar",
        ArtifactKind::PrioritizedBacklog => "backlog",
        ArtifactKind::Other(_) => "art",
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
/// Spawn a cascade rooted on `root_artifact_id`. `max_depth = None`
/// runs the full SDLC pipeline; `Some(n)` bounds the BFS to depth `n`
/// (one click of the workflow card's ▶ uses `Some(1)` so it advances
/// one level at a time).
pub fn spawn_cascade(
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
    max_depth: Option<u32>,
    run_mode: crate::plugins::artifact::cascade::RunMode,
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

    // Walk up the source_artifact_id chain to find the *seed* — the
    // topmost ancestor with no source_artifact_id (typically a
    // user-authored Requirements note). When the user clicks ▶ Play
    // on a child artifact (e.g. an Epic), we still want the cascade's
    // workflow note + chat session to be the SEED's, so all activity
    // under one Requirements tree appears in the same
    // `Cascade: Requirements` tab and the same rail session — no
    // matter which intermediate artifact's Play button kicked off
    // the run. The BFS scope (passed below as `root_artifact_id`)
    // stays the clicked artifact so we only walk that subtree.
    let seed_id = resolve_seed_id_sync(&persistence, root_artifact_id);

    // Mint a fresh chat session per Play click. Two simultaneous
    // cascades — one per click — each get their own rail entry and
    // their own transcript. The label encodes the clicked artifact's
    // kind ([MR] / [epic] / [feat] / [story] / [task] / [imp] / …)
    // and its title so two runs on different artifacts under the same
    // seed are visually distinguishable in the rail; without this they
    // all read `Cascade: <seed>` and pile up indistinguishable.
    let cascade_session_id = Uuid::new_v4();
    let clicked_title = note_repo
        .list_for_project(project_id)
        .ok()
        .and_then(|all| all.into_iter().find(|n| n.id == root_artifact_id))
        .map(|n| n.title)
        .unwrap_or_else(|| short_uuid(root_artifact_id));
    let kind_tag = resolve_artifact_kind_sync(&persistence, root_artifact_id)
        .as_ref()
        .map(artifact_kind_tag)
        .unwrap_or("art");
    let stamp = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Convert seconds-since-epoch to HH:MM:SS local-ish form
        // without pulling in chrono. Wraps every 24h, fine for a
        // labeling suffix.
        let secs_in_day = (now % 86_400) as u64;
        let h = secs_in_day / 3600;
        let m = (secs_in_day % 3600) / 60;
        let s = secs_in_day % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    };
    let session_label = format!("[{}] {} @ {}", kind_tag, clicked_title, stamp);
    let _ = chat_session_repo.create_with_id(
        cascade_session_id,
        operon_store::repos::ChatScope::Project(project_id),
        &session_label,
    );
    let _ = chat_session_repo.touch(cascade_session_id);
    chat_session_version.with_mut(|v| *v = v.saturating_add(1));
    // Register this session as currently running so the companion
    // transcript renderer can show "Claude is working…" until the
    // cascade ends. Removed in the spawn_forever's terminal arms
    // below.
    crate::shell::companion_state::CASCADE_RUNNING_SESSIONS.with_mut(|s| {
        s.insert(cascade_session_id);
    });
    let scope = operon_store::repos::ChatScope::Project(project_id);
    active_scope.set(scope);
    active_session.set(Some(cascade_session_id));
    plugin.bind_session(cascade_session_id, repo_path.clone());

    // Cancellation handle — Stop button on the clicked artifact reads
    // this map. Keyed on the clicked artifact (not the seed) so the
    // Play button on the clicked artifact morphs to Stop while a run
    // is in flight; clicking it cancels exactly the run that started
    // there.
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

    // Resolve a human title for the cascade workflow note from the
    // SEED (not the clicked artifact). All Play invocations under
    // one Requirements seed land in the same `Cascade: <seed title>`
    // workflow note.
    let root_title = note_repo
        .list_for_project(project_id)
        .ok()
        .and_then(|all| all.into_iter().find(|n| n.id == seed_id))
        .map(|n| n.title)
        .unwrap_or_else(|| short_uuid(seed_id));
    let (graph_note_id, _graph_was_created) =
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
    // We deliberately do NOT auto-seed the Cascade-for-Requirements
    // workflow with the natural BA→SA→SDE skill pipeline anymore. The
    // 15 dirty skill nodes that used to appear were noise: the
    // artifact-cascade orchestrator never reads them, they belong to a
    // separate workflow-executor mechanism (`workflow/executor.rs`),
    // and they cluttered the canvas alongside the artifact-snapshot
    // nodes the orchestrator actually produces. Users who want the
    // numbered chain in a workflow can still click `+ Seed pipeline`
    // on the workflow canvas itself (`workflow/view.rs:1287`).

    let note_version_setter = *note_version;
    dioxus::core::spawn_forever(async move {
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
            max_depth,
            cascade_session_id,
            run_mode,
        )
        .await;
        match result {
            Ok(crate::plugins::artifact::cascade::CascadeOutcome::Completed {
                artifacts_produced,
                errors,
            }) => {
                // Surface every level-batched error to the bottom
                // panel's Problems tab. The cascade itself didn't
                // bail (level-batched mode), so the user can review
                // the failures alongside the artifacts that did
                // succeed.
                for err in errors {
                    crate::problems::push_cascade_problem(
                        Some(err.artifact_id),
                        Some(err.skill_title),
                        err.message,
                    );
                }
                // The cascade may have set CASCADE_STATE → Paused
                // already (level-batched cascade_stop). Don't
                // overwrite that with Completed — Paused is the
                // user-visible state we want when there's pending
                // human review. Otherwise mark Completed so the
                // toolbar flips back to ▶ Play.
                let already_paused = matches!(
                    crate::shell::companion_state::CASCADE_STATE
                        .read()
                        .get(&root_artifact_id),
                    Some(crate::shell::companion_state::CascadePhase::Paused { .. })
                );
                if !already_paused {
                    crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                        m.insert(
                            root_artifact_id,
                            crate::shell::companion_state::CascadePhase::Completed {
                                artifacts_produced,
                            },
                        );
                    });
                }
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
                let reason = format!("{e}");
                // Mirror the failure into Problems so the user can
                // see it in the bottom panel even if they're not
                // looking at the artifact view.
                crate::problems::push_cascade_problem(
                    Some(root_artifact_id),
                    None,
                    reason.clone(),
                );
                crate::shell::companion_state::CASCADE_STATE.with_mut(|m| {
                    m.insert(
                        root_artifact_id,
                        crate::shell::companion_state::CascadePhase::Failed { reason },
                    );
                });
            }
        }
        // Drop the cancel token from the registry so a subsequent run
        // creates a fresh token.
        crate::shell::companion_state::CASCADE_CANCEL.with_mut(|m| {
            m.remove(&root_artifact_id);
        });
        // Clear the "Claude is working…" indicator for this session.
        crate::shell::companion_state::CASCADE_RUNNING_SESSIONS.with_mut(|s| {
            s.remove(&cascade_session_id);
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
pub fn chat_session_id_for_cascade(root: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("operon-artifact-cascade:{root}").as_bytes(),
    )
}
