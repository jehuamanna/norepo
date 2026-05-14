//! Artifact-skill runner: spawn claude with a skill's prompt and a
//! source note's body, capture each `.md` file claude writes inside
//! the project's per-source artifacts directory, and import every
//! captured file as a new `NoteKind::Artifact` note linked back to
//! the source.
//!
//! This sits alongside the workflow-canvas executor (which runs
//! single-output skills against a static DAG). Same claude plugin,
//! same chat-message persistence shape; different ingestion model:
//! N output files → N artifact notes, parented to the source so the
//! explorer's tree mirrors the BA → Architect → Engineer hierarchy.

#![cfg(not(target_arch = "wasm32"))]

use futures::StreamExt;
use operon_plugins_claude_code::{ClaudeCodeChatPlugin, ClaudeCodeEvent};
use operon_store::repos::{
    ChatMessageKind, ChatMessageRepository, LocalNote, LocalNoteRepository, LocalProjectRepository,
    NoteKind,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::shell::companion_state::{
    publish_node_live, summarize_tool_input, NodeLiveGuard, NodeLiveTool,
};
use crate::plugins::artifact::aggregation::{
    collect_ancestor_sibling_artifacts, collect_descendant_artifacts,
    collect_master_req_subtree, BundleEntry,
};
use crate::plugins::artifact::frontmatter::{
    rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
};
use crate::plugins::skill::frontmatter::{
    contract as parse_skill_contract, split as split_skill,
};
use crate::plugins::workflow::state::{Edge, Node, NodeStatus, WorkflowGraph};

#[derive(Debug)]
pub enum RunnerError {
    NotFound(String),
    InvalidPath(String),
    Plugin(String),
    Io(std::io::Error),
    /// Pipeline gate refusal: source artifact is not Approved and is
    /// not a root seed. The UI gate normally prevents this; the
    /// runtime check is belt-and-suspenders for non-UI call sites.
    Gated(String),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "not found: {s}"),
            Self::InvalidPath(s) => write!(f, "invalid path: {s}"),
            Self::Plugin(s) => write!(f, "claude: {s}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Gated(s) => write!(f, "gated: {s}"),
        }
    }
}

impl From<std::io::Error> for RunnerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Walk the parent chain from `start_id` looking for the first ancestor
/// whose frontmatter `artifact_kind` equals `target_kind`. Returns
/// `(effective_id, effective_body)` for that ancestor, or `Ok(None)` if
/// no such ancestor exists. Stops at depth 32 to avoid pathological
/// cycles.
///
/// Used by the runner to re-parent skill outputs when the user fires a
/// skill from a wrong-kind source (e.g. clicks Play on an Epic for the
/// architecture skill, which declares `input_kind: master_requirement`).
/// Without this resolution the new artifact gets stamped as a child of
/// the Epic, which shows up in the cascade canvas as
/// `Epic → Architecture` — wrong both topologically and visually.
async fn ascend_to_kind(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    start_id: Uuid,
    target_kind: &str,
) -> Option<(Uuid, String)> {
    let notes = note_repo.list_for_project(project_id).ok()?;
    let by_id: std::collections::HashMap<Uuid, &LocalNote> =
        notes.iter().map(|n| (n.id, n)).collect();
    let mut cursor = by_id.get(&start_id).copied().and_then(|n| n.parent_id);
    let mut steps = 0;
    while let Some(id) = cursor {
        if steps > 32 {
            break;
        }
        steps += 1;
        let Some(node) = by_id.get(&id).copied() else {
            break;
        };
        // Only artifact notes carry the `artifact_kind` frontmatter we
        // match on. A Skill or plain Markdown ancestor is skipped.
        if matches!(node.kind, NoteKind::Artifact) {
            if let Ok(bytes) = persistence.load(&id.to_string()).await {
                if let Ok(body) = String::from_utf8(bytes) {
                    let fm = crate::plugins::artifact::frontmatter::parse(&body);
                    if fm
                        .artifact_kind
                        .as_ref()
                        .map(|k| k.as_str() == target_kind)
                        .unwrap_or(false)
                    {
                        return Some((id, body));
                    }
                }
            }
        }
        cursor = node.parent_id;
    }
    None
}

/// Outcome of one artifact-skill run.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// Note ids of artifacts the run created (in import order).
    pub created_artifact_ids: Vec<Uuid>,
    /// Path under the repo where claude wrote the artifact files (for
    /// debugging — also accessible via `cached_output_path` on each
    /// imported note).
    pub artifacts_dir: PathBuf,
}

/// Entry point. The caller is responsible for having bound the
/// claude plugin to `chat_session_id` against the project's repo
/// path before invoking this — same convention as the workflow
/// executor.
///
/// Phase D: each `chat_message` append also bumps the global
/// `CHAT_MESSAGE_VERSION` signal so the companion's load-effect
/// re-fetches and the transcript ticks live. The signal is a
/// `GlobalSignal` (not context-provided) — see the long comment on
/// its definition in `shell::companion_state` for why a
/// scope-bound `Signal` doesn't work here.
#[allow(clippy::too_many_arguments)]
pub async fn run_skill_on_source(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    persistence: &Arc<dyn Persistence>,
    plugin: &Arc<ClaudeCodeChatPlugin>,
    chat_repo: Option<&Arc<dyn ChatMessageRepository>>,
    chat_session_id: Uuid,
    source_note_id: Uuid,
    skill_note_id: Uuid,
    workflow_node_id: Option<Uuid>,
    cancel: CancellationToken,
) -> Result<RunOutcome, RunnerError> {
    run_skill_on_source_with_revision_notes(
        note_repo,
        project_repo,
        persistence,
        plugin,
        chat_repo,
        chat_session_id,
        source_note_id,
        skill_note_id,
        None,
        Vec::new(),
        workflow_node_id,
        cancel,
    )
    .await
}

/// Variant of `run_skill_on_source` that accepts a caller-supplied
/// `extra_revision_notes` payload (e.g. notes from a Dirty *output*
/// artifact when the user clicks "Re-run" on it). The runner combines
/// these with the source artifact's own `revision_notes` and inlines
/// the result into the skill prompt under
/// `--- refinement notes from user ---`. After a successful import,
/// both note sources are auto-cleared so subsequent re-runs don't
/// replay stale feedback.
///
/// `previous_outputs` carries the prior `(title, body)` pairs of any
/// existing child artifacts that are about to be overwritten or whose
/// subtree just got wiped. The runner inlines these under
/// `--- previous revisions to preserve ---` so the regen prompt can
/// honor the seed-skill convention of appending a new
/// `## Revision N (YYYY-MM-DD)` block and stashing the prior body
/// inside a collapsed `<details>` section rather than discarding
/// history. Pass `Vec::new()` when there's no history to preserve.
///
/// Pass `(None, None)` (i.e., call `run_skill_on_source` instead) when
/// the cascade orchestrator drives the run — cascade-side dirty
/// descendants get wiped before re-runs and don't have a place to
/// surface their notes. The Re-run button path is the primary user
/// motion for this feature today.
#[allow(clippy::too_many_arguments)]
pub async fn run_skill_on_source_with_revision_notes(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    persistence: &Arc<dyn Persistence>,
    plugin: &Arc<ClaudeCodeChatPlugin>,
    chat_repo: Option<&Arc<dyn ChatMessageRepository>>,
    chat_session_id: Uuid,
    source_note_id: Uuid,
    skill_note_id: Uuid,
    extra_revision_notes: Option<(Uuid, String)>,
    previous_outputs: Vec<(String, String)>,
    // M3 live-visibility hook: when the runner is invoked by the
    // workflow cascade for a specific node, this carries the workflow
    // `NodeId` so each AgentEvent can be surfaced on the canvas tile.
    // `None` for per-artifact ▶ runs and the autonomous cascade —
    // those have no workflow-canvas node to update.
    workflow_node_id: Option<Uuid>,
    cancel: CancellationToken,
) -> Result<RunOutcome, RunnerError> {
    // 1. Resolve project + repo_path.
    let project_id = note_repo
        .find_project_for_note(source_note_id)
        .map_err(|e| RunnerError::Plugin(format!("find_project: {e}")))?
        .ok_or_else(|| {
            RunnerError::NotFound(format!("source note {source_note_id} has no project"))
        })?;
    let repo_path: PathBuf = project_repo
        .list()
        .map_err(|e| RunnerError::Plugin(format!("list projects: {e}")))?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| RunnerError::NotFound(format!("project {project_id}")))?
        .repo_path
        .ok_or_else(|| RunnerError::InvalidPath("project has no repo_path bound".into()))?;

    // 2. Load source body + skill body from persistence.
    let source_bytes = persistence
        .load(&source_note_id.to_string())
        .await
        .map_err(|e| RunnerError::NotFound(format!("source body: {e}")))?;
    let mut source_body = String::from_utf8(source_bytes)
        .map_err(|e| RunnerError::Plugin(format!("utf8: {e}")))?;
    let mut source_note_id = source_note_id;

    let skill_bytes = persistence
        .load(&skill_note_id.to_string())
        .await
        .map_err(|e| RunnerError::NotFound(format!("skill body: {e}")))?;
    let skill_body =
        String::from_utf8(skill_bytes).map_err(|e| RunnerError::Plugin(format!("utf8: {e}")))?;

    // 3. Parse skill contract — input/output kind, gate, etc.
    let (skill_fm_lines, _) = split_skill(&skill_body);
    let lines = skill_fm_lines.unwrap_or_default();
    let contract = parse_skill_contract(&lines);

    // 3-pre. Input-kind resolution. When the caller fires this skill
    //   from a source whose `artifact_kind` doesn't match the skill's
    //   declared `input_kind` (e.g. Play clicked on an Epic for the
    //   architecture skill, which wants a `master_requirement`), walk
    //   the parent chain to find the nearest matching ancestor and
    //   re-parent the run there. Without this the artifact gets
    //   stamped as a child of whatever the user clicked, which shows
    //   up in the cascade canvas as `Epic → Architecture` — wrong
    //   both topologically and visually. Sources without
    //   `artifact_kind` frontmatter (plain markdown master-reqs) are
    //   treated as wildcards, matching the view-side picker semantics.
    {
        let candidate_fm = crate::plugins::artifact::frontmatter::parse(&source_body);
        if let Some(target) = contract.input_kind.as_deref() {
            let source_kind_matches = candidate_fm
                .artifact_kind
                .as_ref()
                .map(|k| k.as_str() == target)
                .unwrap_or(true); // None = wildcard
            if !source_kind_matches {
                if let Some((effective_id, effective_body)) = ascend_to_kind(
                    note_repo,
                    persistence,
                    project_id,
                    source_note_id,
                    target,
                )
                .await
                {
                    tracing::info!(
                        target: "operon::artifact",
                        "input_kind mismatch: re-parenting run from {source_note_id} \
                         to ancestor {effective_id} (target kind: {target})"
                    );
                    source_note_id = effective_id;
                    source_body = effective_body;
                } else {
                    return Err(RunnerError::NotFound(format!(
                        "skill expects input_kind={target} but {source_note_id} \
                         has no ancestor with that kind"
                    )));
                }
            }
        }
    }

    // 2a. Pipeline gate: refuse runs when the EFFECTIVE source (after
    //     input-kind resolution above) isn't a runnable status
    //     (Approved or Dirty) unless it's a root seed (no upstream
    //     parent — e.g. a user-authored Requirements note). Mirrors
    //     the UI-side gate in `src/plugins/artifact/view.rs`. Skip
    //     when the source isn't even an Artifact-frontmatter note —
    //     the workflow canvas reuses some of these paths for plain
    //     Markdown sources.
    //
    //     `is_runnable_source` accepts Approved + Dirty so the user
    //     can mark an existing artifact Dirty after editing it and
    //     trigger a re-execution that preserves the existing
    //     children with new revision rows (see cascade.rs source-
    //     dirty regen for the full mechanism).
    let source_fm = crate::plugins::artifact::frontmatter::parse(&source_body);
    if source_fm.artifact_kind.is_some()
        && source_fm.source_artifact_id.is_some()
        && !source_fm.status.is_runnable_source()
    {
        let path_label =
            build_artifact_path_label(note_repo, project_repo, project_id, source_note_id);
        return Err(RunnerError::Gated(format!(
            "source artifact \"{path_label}\" is {} — approve it before running downstream skills",
            source_fm.status.as_str()
        )));
    }

    // 3aa. Normalizer-idempotence gate. A normalizer (input_kind ==
    //      output_kind, output_count: one) overwrites the source
    //      artifact in place rather than producing children. The
    //      cascade's skip-already-produced gate keys off children, so
    //      it can't detect that a normalizer already ran on this
    //      artifact — re-firing it just appends another identical
    //      `## Revision history` row and (with step-mode on) traps the
    //      cascade in a no-op checkpoint loop. Skip whenever the
    //      source's `source_skill_id` is already set: that means some
    //      upstream skill produced it (BA-produced artifact = canonical
    //      by construction, with the producer pointer preserved by
    //      `import_normalizer_rewrite` per commit 68c349c) or this
    //      normalizer has already run (subsequent runs would just
    //      duplicate revision rows). Hand-authored artifacts
    //      (`source_skill_id.is_none()`) still trigger the normalizer
    //      once. Re-canonicalising a hand-edited descendant requires
    //      marking the cascade root Dirty (existing
    //      `regenerate-on-dirty` flow at `cascade.rs:380-431`).
    if is_normalizer_contract(&contract)
        && decide_normalizer_skip(source_fm.source_skill_id)
    {
        tracing::debug!(
            target: "operon::artifact",
            "normalizer-idempotence: skipping skill {skill_note_id} on \
             {source_note_id} (source_skill_id already set — body is canonical)"
        );
        return Ok(RunOutcome {
            created_artifact_ids: Vec::new(),
            artifacts_dir: repo_path.clone(),
        });
    }

    // 3a. Aggregator skills: collect every descendant artifact under
    //     the source seed whose `artifact_kind` matches the declared
    //     `aggregate:` kind. The collected (title, body) pairs are
    //     inlined into the prompt so the LLM sees every Task (or
    //     every Plan, etc.) at once. Walks the tree breadth-first
    //     under `source_note_id` — siblings of the seed are NOT
    //     pulled in.
    let aggregated: Vec<(String, String)> =
        if let Some(kind) = contract.aggregate.as_deref() {
            collect_descendant_artifacts(note_repo, persistence, project_id, source_note_id, kind)
                .await
        } else {
            Vec::new()
        };

    // Phase D: master-requirement subtree bundling. When the source
    // is a master_requirement, walk its descendant tree and pull
    // every Markdown note, nested master_requirement artifact, and
    // Image attachment into the prompt as supporting context. This
    // is the "bundle the whole subtree into one input" semantic the
    // user picked during the three-tier design — discovery is rarely
    // a single document, it's a folder of design docs, mockups, and
    // user-flow notes.
    //
    // Distinct from `aggregated` (which collects same-kind artifacts
    // matching the skill's declared `aggregate:` field) and
    // `inherited` (ancestor-chain context). This is specifically the
    // master_req's own user-authored children, not generated
    // siblings.
    let bundle: Vec<BundleEntry> = if source_fm
        .artifact_kind
        .as_ref()
        .map(|k| k.as_str() == "master_requirement")
        .unwrap_or(false)
    {
        collect_master_req_subtree(note_repo, persistence, project_id, source_note_id).await
    } else {
        Vec::new()
    };

    // 3b. Inheritance: walk the **ancestor chain** from the source
    //     upward and inline every sibling artifact whose
    //     `artifact_kind` matches the declared `inherit:` kind. Lets a
    //     skill pull design context produced upstream — e.g. an SDE
    //     skill on a Task pulls the parent Story's LLD plan and the
    //     grandparent Feature's HLD plan into its prompt. Empty when
    //     the contract doesn't declare `inherit:` or no matching
    //     ancestors-sibling artifacts exist.
    let inherited: Vec<(String, String)> =
        if let Some(kind) = contract.inherit.as_deref() {
            collect_ancestor_sibling_artifacts(
                note_repo,
                persistence,
                project_id,
                source_note_id,
                kind,
            )
            .await
        } else {
            Vec::new()
        };

    // 4. Where claude is going to write the new artifact files.
    //    Migration 018: this is a per-run tempdir under `.operon/`, not
    //    a UUID-named directory under `.operon/artifacts/`. Canonical
    //    artifact bodies live at
    //    `.operon/artifacts/<resolved-slug-path>/index.md` (written by
    //    `ArtifactPersistence` during `import_produced_artifacts`); the
    //    tempdir is just a buffer Claude streams files into and is
    //    auto-cleaned when `_staging_guard` drops at function end.
    let operon_dir = repo_path.join(".operon");
    std::fs::create_dir_all(&operon_dir)?;
    let _staging_guard = tempfile::tempdir_in(&operon_dir)?;
    let artifacts_dir = _staging_guard.path().to_path_buf();
    let run_started_at = SystemTime::now();

    // 5. Pre-snapshot the directory so we only import files claude
    //    creates *during this run*, not pre-existing leftovers from a
    //    prior run that the user already imported.
    let existing: HashSet<PathBuf> = list_artifact_files(&artifacts_dir);

    // 6. Build the prompt that claude will see. Combine the source
    //    artifact's `revision_notes` (set by the user on the artifact
    //    they're refining) with any caller-supplied
    //    `extra_revision_notes` (e.g. notes lifted off a Dirty output
    //    artifact when the user clicked "Re-run" on it). Both reach
    //    the LLM under a single `--- refinement notes from user ---`
    //    fence so the model treats them as one priority block.
    let combined_notes =
        combine_revision_notes(source_fm.revision_notes.as_deref(), extra_revision_notes.as_ref());
    let prompt = build_prompt(
        &source_body,
        &skill_body,
        &artifacts_dir,
        &contract,
        source_note_id,
        skill_note_id,
        &aggregated,
        &inherited,
        combined_notes.as_deref(),
        &previous_outputs,
        &bundle,
    );

    // 7. Persist the prompt as a User message (transcript visibility).
    if let Some(repo) = chat_repo {
        if let Err(e) = repo.append(
            chat_session_id,
            ChatMessageKind::User,
            None,
            &serde_json::json!({ "text": prompt.clone() }),
        ) {
            tracing::warn!(
                target: "operon::artifact",
                "persist user prompt to {chat_session_id}: {e:?}"
            );
        } else {
            bump_message_version();
        }
    }

    // 8. Bind the chat session to the project's repo before sending.
    //    Single-skill spawn from the artifact view already binds in
    //    `spawn_runner` (view.rs), but the cascade orchestrator calls
    //    this fn directly with a fresh per-source-artifact session id
    //    and skipped the bind — claude's plugin then refused
    //    `send_rich` with "session is not bound to a repository".
    //    `bind_session` is idempotent, so binding here covers both
    //    flows without breaking the single-skill path.
    plugin.bind_session(chat_session_id, repo_path.clone());

    // 8a. Wire up the inline-permission-prompt MCP bridge so that any
    //     Bash command claude wants to run (which `acceptEdits` does
    //     NOT auto-approve — only file edits do) surfaces as a
    //     clickable prompt in the active companion chat instead of
    //     silently denying. Idempotent per session; safe to call on
    //     every runner invocation.
    if let Err(e) = crate::shell::companion_state::ensure_session_bridge(
        plugin,
        chat_session_id,
        repo_path.clone(),
    )
    .await
    {
        tracing::warn!(
            target: "operon::permission",
            "ensure_session_bridge({chat_session_id}): {e}"
        );
    }

    // 9. Run claude. The runner forces `acceptEdits` on this
    //    session so its automated Write tool calls don't hang
    //    waiting for stdin approval — even when the user's global
    //    permission picker is set to "default". Normal companion
    //    chats keep using whatever the user picked, since they
    //    don't set a per-session override.
    plugin.set_session_permission_mode(chat_session_id, Some("acceptEdits".into()));
    // Pass the caller's cancellation token straight through to the
    // plugin so the in-flight `claude` subprocess dies when the user
    // clicks Stop. `drive_stream` does `proc.child.start_kill()` on
    // `ct.cancelled()` (claude-code/src/stream.rs:36-43); without
    // routing the outer token here, the plugin watched a fresh token
    // that no Stop click could ever cancel, so the subprocess kept
    // running for up to 2 minutes after the user gave up.
    // `CancellationToken` clones share state — when the cascade's
    // outer token cancels, this clone fires too.
    let ct = cancel.clone();

    // M3 live visibility — published only when the runner is invoked
    // for a workflow canvas node. Clear any prior run's state and arm
    // a drop-guard so a cancelled task doesn't leave the tile stuck
    // showing the last tool call.
    if let Some(nid) = workflow_node_id {
        crate::shell::companion_state::NODE_LIVE_STATE.with_mut(|m| {
            m.remove(&nid);
        });
    }
    let mut node_live_guard = workflow_node_id.map(NodeLiveGuard::armed);

    let mut rx = plugin
        .send_rich(prompt, chat_session_id, ct)
        .await
        .map_err(|e| {
            if let Some(nid) = workflow_node_id {
                let reason = format!("send_rich: {e}");
                publish_node_live(nid, |s| s.last_error = Some(reason));
                if let Some(g) = node_live_guard.as_mut() {
                    g.disarm();
                }
            }
            RunnerError::Plugin(format!("send_rich: {e}"))
        })?;
    let mut assistant_buf = String::new();
    while let Some(ev) = rx.next().await {
        // Persist event to the rail's chat_message (mirroring the
        // workflow executor's pattern).
        if let Some(repo) = chat_repo {
            let appended = persist_event(repo, chat_session_id, &ev, &mut assistant_buf);
            if appended {
                bump_message_version();
            }
        }
        // Streaming artifact import: as soon as Claude calls `Write`
        // for a path under the per-source `artifacts_dir`, land that
        // single file in the explorer immediately. Lets the user
        // watch Features / Tasks pop in one-by-one for `output_count:
        // many` skills instead of seeing nothing for the whole run
        // and then 5 artifacts at the end. The end-of-run scan +
        // import remains a safety net (idempotent dedup at
        // `import_produced_artifacts` keeps it a no-op for files
        // already imported).
        // M3 live publish: surface tool/thinking activity on the
        // workflow tile (when invoked from a workflow node).
        if let Some(nid) = workflow_node_id {
            match &ev {
                ClaudeCodeEvent::ToolUse { id, name, input } => {
                    let summary = summarize_tool_input(name, input);
                    let write_file = if name == "Write" {
                        input
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .map(|p| {
                                std::path::Path::new(p)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(p)
                                    .to_string()
                            })
                    } else {
                        None
                    };
                    let tool = NodeLiveTool {
                        id: id.clone(),
                        name: name.clone(),
                        summary,
                    };
                    publish_node_live(nid, move |s| {
                        s.thinking = false;
                        s.active_tool = Some(tool);
                        if let Some(f) = write_file {
                            s.last_write_file = Some(f);
                        }
                    });
                }
                ClaudeCodeEvent::ToolResult { tool_use_id, .. } => {
                    let id = tool_use_id.clone();
                    publish_node_live(nid, move |s| {
                        if s.active_tool
                            .as_ref()
                            .map(|t| t.id == id)
                            .unwrap_or(false)
                        {
                            s.active_tool = None;
                        }
                    });
                }
                ClaudeCodeEvent::Thinking(_) => {
                    publish_node_live(nid, |s| {
                        s.thinking = true;
                        s.active_tool = None;
                    });
                }
                _ => {}
            }
        }

        if let ClaudeCodeEvent::ToolUse { name, input, .. } = &ev {
            if name == "Write" {
                if let Some(file_path) =
                    input.get("file_path").and_then(|v| v.as_str())
                {
                    let path = std::path::Path::new(file_path);
                    if path.parent() == Some(artifacts_dir.as_path()) {
                        // Best-effort flush: Claude's Write tool
                        // usually has the bytes on disk by this
                        // event, but there's no hard ordering
                        // guarantee — write the inlined content if
                        // the file is missing.
                        if !path.exists() {
                            if let Some(content) =
                                input.get("content").and_then(|v| v.as_str())
                            {
                                let _ = std::fs::write(path, content);
                            }
                        }
                        let imported = import_produced_artifacts(
                            note_repo,
                            persistence,
                            project_id,
                            source_note_id,
                            skill_note_id,
                            &contract,
                            std::slice::from_ref(&path.to_path_buf()),
                        )
                        .await;
                        if !imported.is_empty() {
                            crate::shell::companion_state::LOCAL_NOTE_VERSION
                                .with_mut(|v| *v = v.saturating_add(1));
                        }
                    }
                }
            }
        }
        match ev {
            ClaudeCodeEvent::Done { .. } => {
                if let Some(nid) = workflow_node_id {
                    crate::shell::companion_state::NODE_LIVE_STATE.with_mut(|m| {
                        m.remove(&nid);
                    });
                    if let Some(g) = node_live_guard.as_mut() {
                        g.disarm();
                    }
                }
                break;
            }
            ClaudeCodeEvent::Error(msg) => {
                if let Some(nid) = workflow_node_id {
                    let reason = msg.clone();
                    publish_node_live(nid, move |s| {
                        s.thinking = false;
                        s.active_tool = None;
                        s.last_error = Some(reason);
                    });
                    if let Some(g) = node_live_guard.as_mut() {
                        g.disarm();
                    }
                }
                if let Some(repo) = chat_repo {
                    let _ = repo.append(
                        chat_session_id,
                        ChatMessageKind::System,
                        None,
                        &serde_json::json!({ "text": format!("error: {msg}") }),
                    );
                    bump_message_version();
                }
                return Err(RunnerError::Plugin(msg));
            }
            _ => {}
        }
    }
    // Flush any leftover assistant text the persist helper buffered.
    // Body shape MUST match what `transcript_item_from_message`
    // expects for Assistant kind: `{ "body": "<text>" }`. Earlier we
    // wrote `{ "text": ... }` and every assistant message was
    // silently filtered out of the rail's transcript.
    if let Some(repo) = chat_repo {
        if !assistant_buf.is_empty() {
            let _ = repo.append(
                chat_session_id,
                ChatMessageKind::Assistant,
                None,
                &serde_json::json!({ "body": std::mem::take(&mut assistant_buf) }),
            );
            bump_message_version();
        }
    }

    // 9. Scan the artifacts dir for files that are either new or
    //    have an mtime past `run_started_at` (claude may have
    //    overwritten an existing file on a re-run).
    let produced = scan_produced_artifact_files(&artifacts_dir, &existing, run_started_at);

    // 10. Import each produced file as an Artifact note under the
    //     source. Delegates to the shared `import_produced_artifacts`
    //     helper so the workflow-canvas executor can use the same
    //     codepath (Phase 3 of the parity port).
    let created_ids: Vec<Uuid> = import_produced_artifacts(
        note_repo,
        persistence,
        project_id,
        source_note_id,
        skill_note_id,
        &contract,
        &produced,
    )
    .await;

    // 10a. Clear `revision_notes` on the artifacts whose notes we
    //      just consumed in the prompt — successful regeneration
    //      means the user's feedback was applied; replaying it on
    //      the next run would be wrong. Skipped on a zero-import
    //      run so a failed regeneration leaves the notes intact for
    //      retry.
    if !created_ids.is_empty() && combined_notes.is_some() {
        if source_fm.revision_notes.is_some() {
            clear_revision_notes(persistence, source_note_id, &source_body).await;
        }
        if let Some((extra_id, _)) = extra_revision_notes.as_ref() {
            clear_revision_notes_by_id(persistence, *extra_id).await;
        }
    }

    // 11. Workflow emission: removed. Prioritization skills used to
    //     declare `emit_workflow: true` and the runner would write
    //     a sibling `NoteKind::Workflow` snapshot of the prioritized
    //     tasks alongside the produced backlog. That side-effect
    //     was disabled globally — users only want one workflow
    //     note per cascade root (the live `Cascade: <seed>` canvas
    //     populated by `CascadeGraphWriter`); the auto-emitted
    //     prioritized-tasks graphs were noise. The
    //     `emit_workflow` frontmatter field still parses (no-op)
    //     so existing skill files keep loading. Helper
    //     `emit_workflow_for_backlog` deleted along with this
    //     callsite.

    Ok(RunOutcome {
        created_artifact_ids: created_ids,
        artifacts_dir,
    })
}

/// Extract titles listed under the backlog artifact's `## Priority
/// order` heading. Tolerant: returns an empty Vec when the section is
/// absent. Recognizes ordered (`1. T001`) and unordered (`- T001`)
/// markers; the first whitespace-delimited token after the marker is
/// taken as the lookup slug, matching the cascade graph's depends-on
/// resolution rules.
pub fn parse_priority_order(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("##") {
            let heading = trimmed.trim_start_matches('#').trim().to_lowercase();
            in_section = heading == "priority order"
                || heading == "priorities"
                || heading == "priority";
            continue;
        }
        if !in_section {
            continue;
        }
        // Strip ordered-list "1." / "12)" prefix and unordered
        // bullet markers, then read the first token.
        let after_number = strip_list_marker(trimmed);
        if after_number.is_empty() || after_number == trimmed {
            // Plain prose / blank lines inside the section are
            // ignored; only listed bullets count.
            continue;
        }
        let token = after_number
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches([',', '.', ':', ';', ')']);
        if token.is_empty() || token.eq_ignore_ascii_case("none") {
            continue;
        }
        out.push(token.to_string());
    }
    out
}

/// Strip a leading list marker. Returns the original slice (unchanged)
/// when no marker is present so the caller can detect "this isn't a
/// list line" vs "this is a list line with empty content".
fn strip_list_marker(line: &str) -> &str {
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return rest.trim_start();
    }
    // Ordered: "12. foo" or "12) foo"
    let digits: String = line.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &line[digits.len()..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            return rest.trim_start();
        }
    }
    line
}

#[allow(clippy::too_many_arguments)]
fn build_prompt(
    source_body: &str,
    skill_body: &str,
    artifacts_dir: &Path,
    contract: &crate::plugins::skill::frontmatter::SkillContract,
    source_id: Uuid,
    skill_id: Uuid,
    aggregated: &[(String, String)],
    inherited: &[(String, String)],
    revision_notes: Option<&str>,
    previous_outputs: &[(String, String)],
    bundle: &[BundleEntry],
) -> String {
    let mut buf = String::new();
    buf.push_str(
        "You are running an SDLC artifact-producing skill. Read the source\n\
         artifact below, follow the skill instructions, and write each output\n\
         artifact as a SEPARATE markdown file using the Write tool.\n\n",
    );

    buf.push_str(&format!(
        "Each output file must be written under the absolute directory:\n  {}\n\
         using a short kebab-case filename like `epic-user-auth.md`.\n\n",
        artifacts_dir.display()
    ));

    let kind_label = contract.output_kind.as_deref().unwrap_or("artifact");
    buf.push_str(&format!(
        "Each file MUST start with this YAML frontmatter (and nothing else\n\
         before it):\n\n\
         ```yaml\n\
         ---\n\
         artifact_kind: {kind_label}\n\
         status: pending\n\
         source_artifact_id: {source_id}\n\
         source_skill_id: {skill_id}\n\
         ---\n\
         ```\n\n\
         Then the artifact body in markdown. The first heading should match\n\
         the file name (in human-readable form).\n\n"
    ));

    buf.push_str("--- skill body ---\n");
    buf.push_str(skill_body.trim_end());
    buf.push_str("\n--- /skill body ---\n\n");

    buf.push_str("--- source artifact body ---\n");
    buf.push_str(source_body.trim_end());
    buf.push_str("\n--- /source artifact body ---\n\n");

    // Phase D: master-requirement subtree bundle. Markdown notes and
    // nested master_requirement artifacts authored under the source
    // are inlined here as supporting context. Image attachments are
    // listed by path — the model can `Read` them if it needs visual
    // detail. Order is depth-first by sibling_index so the prompt
    // mirrors the explorer tree.
    if !bundle.is_empty() {
        let text_count = bundle
            .iter()
            .filter(|e| matches!(e, BundleEntry::Text { .. }))
            .count();
        let image_count = bundle
            .iter()
            .filter(|e| matches!(e, BundleEntry::Image { .. }))
            .count();
        buf.push_str(&format!(
            "--- master-requirement subtree ({text_count} text + {image_count} image) ---\n\
             These are notes the user authored under the source master\n\
             requirement (nested master-requirements, design markdown,\n\
             attached images). Treat them as part of the spec the\n\
             requirement is describing.\n\n"
        ));
        for entry in bundle {
            match entry {
                BundleEntry::Text { title, body } => {
                    buf.push_str(&format!("--- subtree: {title} ---\n"));
                    buf.push_str(body.trim_end());
                    buf.push_str(&format!("\n--- /subtree: {title} ---\n\n"));
                }
                BundleEntry::Image { title, path } => {
                    buf.push_str(&format!(
                        "--- subtree image: {title} ---\n\
                         path: {}\n\
                         (Use the Read tool to fetch this image if visual context is needed.)\n\
                         --- /subtree image: {title} ---\n\n",
                        path.display()
                    ));
                }
            }
        }
        buf.push_str("--- /master-requirement subtree ---\n\n");
    }

    if !aggregated.is_empty() {
        let kind = contract.aggregate.as_deref().unwrap_or("artifact");
        buf.push_str(&format!(
            "--- aggregated {kind} artifacts ({n} total) ---\n\
             Every {kind} artifact under the source seed is inlined below. Use\n\
             these as the canonical input set; do NOT consult the filesystem\n\
             for additional artifacts of this kind.\n\n",
            n = aggregated.len()
        ));
        for (title, body) in aggregated {
            buf.push_str(&format!("--- artifact: {title} ---\n"));
            buf.push_str(body.trim_end());
            buf.push_str(&format!("\n--- /artifact: {title} ---\n\n"));
        }
        buf.push_str(&format!("--- /aggregated {kind} artifacts ---\n\n"));
    }

    if !inherited.is_empty() {
        let kind = contract.inherit.as_deref().unwrap_or("artifact");
        buf.push_str(&format!(
            "--- inherited {kind} artifacts from ancestor chain ({n} total) ---\n\
             These are design / context artifacts produced upstream of the\n\
             source (siblings of one of its ancestors). Treat them as\n\
             authoritative inputs that scope the work you're about to do.\n\
             If the source artifact and an inherited artifact disagree,\n\
             flag the contradiction in your output rather than silently\n\
             choosing one.\n\n",
            n = inherited.len()
        ));
        for (title, body) in inherited {
            buf.push_str(&format!("--- artifact: {title} ---\n"));
            buf.push_str(body.trim_end());
            buf.push_str(&format!("\n--- /artifact: {title} ---\n\n"));
        }
        buf.push_str(&format!("--- /inherited {kind} artifacts ---\n\n"));
    }

    if !previous_outputs.is_empty() {
        buf.push_str(&format!(
            "--- previous revisions to preserve ({n} total) ---\n\
             These are the prior bodies of the artifacts you're about to\n\
             regenerate. The seed-skill convention is to PRESERVE history\n\
             rather than discard it:\n\
             \n\
             1. Generate the new revision's body above any prior content.\n\
             2. Move the previous body's content into a collapsed\n\
                `<details><summary>Revision N (YYYY-MM-DD)</summary>` block\n\
                at the bottom of the new file. Stack older revisions below\n\
                newer ones inside their own `<details>` blocks.\n\
             3. Add a new `## Revision history` row dated today summarising\n\
                what changed in this regeneration. Keep every prior row\n\
                verbatim.\n\
             \n\
             Match each previous body to the new file you write by title:\n\
             reuse the same kebab-case filename so the engine's title-based\n\
             dedup overwrites in place. If you choose to rename, drop a\n\
             pointer row in the new file's revision history.\n\n",
            n = previous_outputs.len()
        ));
        for (title, body) in previous_outputs {
            buf.push_str(&format!("--- previous: {title} ---\n"));
            buf.push_str(body.trim_end());
            buf.push_str(&format!("\n--- /previous: {title} ---\n\n"));
        }
        buf.push_str("--- /previous revisions to preserve ---\n\n");
    }

    if let Some(notes) = revision_notes {
        let trimmed = notes.trim();
        if !trimmed.is_empty() {
            buf.push_str(
                "--- refinement notes from user ---\n\
                 The user has explicitly requested the following adjustments\n\
                 for this regeneration. Treat them as priority guidance over\n\
                 the source artifact body when they conflict. After applying\n\
                 them, write the new artifact(s) as instructed above.\n\n",
            );
            buf.push_str(trimmed);
            buf.push_str("\n--- /refinement notes from user ---\n\n");
        }
    }

    buf.push_str(
        "When done, do NOT echo the artifact contents back to the user — the\n\
         engine reads them from disk. A short summary of how many artifacts\n\
         you produced is enough.\n",
    );
    buf
}

/// Combine source-side and caller-supplied refinement notes into a
/// single payload for `build_prompt`. Both are trimmed; empty inputs
/// are dropped; when both are present they're concatenated with a
/// `[from <kind>]:` label so the model can tell them apart.
fn combine_revision_notes(
    source_notes: Option<&str>,
    extra: Option<&(Uuid, String)>,
) -> Option<String> {
    let src = source_notes.map(str::trim).filter(|s| !s.is_empty());
    let extra_text = extra
        .map(|(_, s)| s.as_str().trim())
        .filter(|s| !s.is_empty());
    match (src, extra_text) {
        (None, None) => None,
        (Some(s), None) => Some(s.to_string()),
        (None, Some(e)) => Some(e.to_string()),
        (Some(s), Some(e)) => Some(format!(
            "[from source artifact]:\n{s}\n\n[from output artifact]:\n{e}"
        )),
    }
}

/// Build a human-readable WPN-style path for an artifact: `Project /
/// Parent / This Title`. Used by the gate-refusal error so the user
/// sees what to approve, not a raw UUID. Falls back to the UUID for
/// any segment that can't be resolved (missing project row, missing
/// note row, broken parent chain) — never panics, never blocks the
/// caller.
fn build_artifact_path_label(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    project_id: Uuid,
    artifact_id: Uuid,
) -> String {
    let project_name = project_repo
        .list()
        .ok()
        .and_then(|all| all.into_iter().find(|p| p.id == project_id))
        .map(|p| p.name)
        .unwrap_or_else(|| project_id.to_string());

    let notes = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return format!("{project_name} / {artifact_id}"),
    };
    let by_id: std::collections::HashMap<Uuid, &LocalNote> =
        notes.iter().map(|n| (n.id, n)).collect();

    // Walk parent chain, capping the depth to avoid pathological cycles.
    let mut titles: Vec<String> = Vec::new();
    let mut current = by_id.get(&artifact_id).copied();
    let mut steps = 0;
    while let Some(n) = current {
        titles.push(n.title.clone());
        if steps > 32 {
            break;
        }
        current = n.parent_id.and_then(|p| by_id.get(&p).copied());
        steps += 1;
    }
    titles.reverse();
    if titles.is_empty() {
        format!("{project_name} / {artifact_id}")
    } else {
        format!("{project_name} / {}", titles.join(" / "))
    }
}

/// Import a list of skill-produced `.md` files as `NoteKind::Artifact`
/// notes under `source_note_id`. Shared between the cascade orchestrator
/// (`run_skill_on_source` above) and the workflow-canvas executor
/// (Phase 3 of the parity port).
///
/// Behavior, per file:
/// - Read the file body from disk; skip files that fail to read.
/// - Derive a title from the file stem.
/// - Dedup against existing sibling Artifact notes under the source
///   (parent_id match + same title). On hit, reuse that row id; on
///   miss, create a new `NoteKind::Artifact` row parented to the
///   source. This keeps the explorer tree stable across re-runs —
///   without it, every Re-run / Revise cycle would duplicate every
///   child under the same parent.
/// - Patch artifact frontmatter so `artifact_kind` falls back to the
///   skill contract's `output_kind`, `status` reflects the skill's
///   `gate`, and `source_artifact_id` / `source_skill_id` are always
///   authoritative regardless of what the model wrote.
/// - Save the rewritten body via Persistence.
///
/// Returns the row ids of every successfully imported file (in input
/// order). Errors during read / create / save are logged via eprintln
/// and skip the offending file rather than aborting the batch — same
/// resilience as the inlined version.
pub async fn import_produced_artifacts(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    source_note_id: Uuid,
    skill_note_id: Uuid,
    contract: &crate::plugins::skill::frontmatter::SkillContract,
    produced: &[PathBuf],
) -> Vec<Uuid> {
    // Normalizer branch. When a skill declares the same input and
    // output kind with `output_count: one` (e.g. `02n-ba-normalize-
    // epics`), the produced file is a *rewrite* of the source
    // artifact, not a fresh child. Overwrite the source body in
    // place, preserve the source's existing parent linkage, and
    // skip child-note creation. Returns the source id so the
    // revision-notes clear logic upstream still fires.
    //
    // Inline-clarification invariant (skills 02–09): a clarification
    // `.mdx` file emitted mid-run carries `artifact_kind:
    // clarification`, which never equals the producing skill's
    // `contract.output_kind` (epic / feature / story / task /
    // architecture / …). `is_normalizer_contract` therefore returns
    // `false` and the clarification falls through to the child-
    // creating branch below — exactly what we want, since the
    // clarification is a sibling of the skill's normal output, not a
    // rewrite of the source.
    if is_normalizer_contract(contract) {
        return import_normalizer_rewrite(
            persistence,
            source_note_id,
            skill_note_id,
            contract,
            produced,
        )
        .await;
    }
    let existing_siblings: Vec<LocalNote> = note_repo
        .list_for_project(project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            n.parent_id == Some(source_note_id) && matches!(n.kind, NoteKind::Artifact)
        })
        .collect();

    let mut created_ids: Vec<Uuid> = Vec::new();
    for file in produced {
        let body = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("operon: artifact import skipped {} ({e})", file.display());
                continue;
            }
        };
        let title = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("artifact")
            .to_string();
        let existing_id = existing_siblings
            .iter()
            .find(|n| n.title == title)
            .map(|n| n.id);
        let row_id = match existing_id {
            Some(id) => id,
            None => match note_repo.create_with_kind(
                project_id,
                Some(source_note_id),
                &title,
                NoteKind::Artifact,
            ) {
                Ok(r) => r.id,
                Err(e) => {
                    eprintln!("operon: artifact create_with_kind failed: {e}");
                    continue;
                }
            },
        };
        let mut fm = crate::plugins::artifact::frontmatter::parse(&body);
        if fm.artifact_kind.is_none() {
            fm.artifact_kind = contract
                .output_kind
                .as_deref()
                .map(ArtifactKind::parse);
        }
        fm.status = match contract.gate {
            crate::plugins::skill::frontmatter::SkillGate::Auto => {
                ArtifactStatus::Approved
            }
            crate::plugins::skill::frontmatter::SkillGate::Approval => {
                ArtifactStatus::Pending
            }
        };
        fm.source_artifact_id = Some(source_note_id);
        fm.source_skill_id = Some(skill_note_id);
        let final_body = rewrite_artifact_fm(&body, &fm);
        if let Err(e) = persistence
            .save(&row_id.to_string(), final_body.as_bytes())
            .await
        {
            eprintln!("operon: artifact persistence save failed: {e}");
            continue;
        }
        created_ids.push(row_id);
    }

    // Phase E: when the skill produced architecture_review notes,
    // flip `needs_review: true` on the parent architecture so the
    // explorer + canvas badges pick it up. The source for an
    // architecture-review run is the architecture artifact itself
    // (input_kind: architecture), so `source_note_id` IS the
    // parent we need to mark. Only flip when we actually imported at
    // least one review — empty runs (no produced files) shouldn't
    // raise the flag.
    if !created_ids.is_empty()
        && contract.output_kind.as_deref() == Some("architecture_review")
    {
        flip_needs_review_on(persistence, source_note_id, true).await;
    }

    created_ids
}

/// Phase E: rewrite the artifact at `note_id` to set
/// `needs_review = value`, preserving the rest of the body and
/// frontmatter. Used by `import_produced_artifacts` to raise the
/// flag on an architecture when a fresh review lands, and by the
/// view's approve/reject handlers to clear it when no Pending /
/// Dirty reviews remain. Errors are logged and swallowed — flipping
/// the flag is best-effort UI signaling, not load-bearing semantics.
pub async fn flip_needs_review_on(
    persistence: &Arc<dyn Persistence>,
    note_id: Uuid,
    value: bool,
) {
    let bytes = match persistence.load(&note_id.to_string()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "flip_needs_review: load {note_id} failed: {e}"
            );
            return;
        }
    };
    let body = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "flip_needs_review: utf8 {note_id}: {e}"
            );
            return;
        }
    };
    let mut fm = crate::plugins::artifact::frontmatter::parse(&body);
    if fm.needs_review == value {
        return; // no-op, avoid touching mtime
    }
    fm.needs_review = value;
    let rewritten = rewrite_artifact_fm(&body, &fm);
    if let Err(e) = persistence
        .save(&note_id.to_string(), rewritten.as_bytes())
        .await
    {
        tracing::warn!(
            target: "operon::artifact",
            "flip_needs_review: save {note_id} failed: {e}"
        );
    }
}

/// `true` when a skill contract describes an in-place rewrite: the
/// input and output kinds match and exactly one artifact is produced.
/// Used by `import_produced_artifacts` to switch from the
/// child-creating branch to the source-overwriting branch. Empty
/// `input_kind` / `output_kind` (rare; usually means the skill
/// hand-rolled the frontmatter) doesn't qualify — both fields must
/// be set and equal.
/// Decide whether a normalizer-skill run on the source artifact should
/// be skipped as redundant. Returns `true` whenever the source's
/// `source_skill_id` is already set — meaning some upstream skill has
/// already produced (or normalized) the artifact, so its body is
/// canonical and re-running the normalizer would only append an
/// identical `## Revision history` row.
///
/// Hand-authored artifacts (`source_skill_id.is_none()`) return
/// `false` so the normalizer runs once to canonicalise the body.
///
/// Pure function — split off the runner's normalizer-idempotence gate
/// in `run_skill_on_source_with_revision_notes` so the rule is
/// trivially unit-testable.
pub fn decide_normalizer_skip(source_skill_id: Option<Uuid>) -> bool {
    source_skill_id.is_some()
}

pub fn is_normalizer_contract(
    contract: &crate::plugins::skill::frontmatter::SkillContract,
) -> bool {
    if !matches!(
        contract.output_count,
        crate::plugins::skill::frontmatter::SkillOutputCount::One
    ) {
        return false;
    }
    match (
        contract.input_kind.as_deref(),
        contract.output_kind.as_deref(),
    ) {
        (Some(i), Some(o)) if !i.is_empty() && i == o => true,
        _ => false,
    }
}

/// Apply a normalizer skill's first produced file as an in-place
/// rewrite of the source artifact: keep the source's existing parent
/// linkage and `source_artifact_id`, swap in the new body, refresh
/// `source_skill_id` to the normalizer that just ran, and pick the
/// status from the skill's gate (Auto → Approved, Approval → Pending).
/// Returns `[source_note_id]` on success, empty Vec otherwise.
///
/// Multi-file output is silently truncated to the first file — a
/// normalizer that emits multiple `.md` files is a skill-prompt bug
/// (the seed instructs `Write` exactly once), and child-creating
/// behavior for the extras would be worse than dropping them.
async fn import_normalizer_rewrite(
    persistence: &Arc<dyn Persistence>,
    source_note_id: Uuid,
    skill_note_id: Uuid,
    contract: &crate::plugins::skill::frontmatter::SkillContract,
    produced: &[PathBuf],
) -> Vec<Uuid> {
    let Some(first) = produced.first() else {
        return Vec::new();
    };
    let new_body = match std::fs::read_to_string(first) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "normalizer rewrite skipped: read {} failed: {e}",
                first.display()
            );
            return Vec::new();
        }
    };
    // Pull existing source frontmatter so we can preserve the
    // parent linkage (the source's own `source_artifact_id`) — the
    // normalizer is a sibling-level rewrite, not a re-parenting.
    let existing_bytes = match persistence.load(&source_note_id.to_string()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "normalizer rewrite skipped: load existing source {source_note_id} failed: {e}"
            );
            return Vec::new();
        }
    };
    let existing_body = match String::from_utf8(existing_bytes) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "normalizer rewrite skipped: existing source {source_note_id} utf8: {e}"
            );
            return Vec::new();
        }
    };
    let existing_fm = crate::plugins::artifact::frontmatter::parse(&existing_body);
    // Build the final frontmatter: kind from contract (== input kind
    // == output kind), parent linkage from existing source, producer
    // pointer resolved against the existing source (preserve a real
    // upstream producer when present — see
    // `resolve_normalizer_source_skill_id` for why), status from
    // skill gate so an auto-gate normalizer leaves the artifact
    // ready to feed downstream, and an approval-gate one parks it
    // Pending for a human review pass.
    let mut fm = crate::plugins::artifact::frontmatter::parse(&new_body);
    if fm.artifact_kind.is_none() {
        fm.artifact_kind = contract
            .output_kind
            .as_deref()
            .map(ArtifactKind::parse);
    }
    fm.source_artifact_id = existing_fm.source_artifact_id;
    let resolved_source_skill_id = resolve_normalizer_source_skill_id(
        persistence,
        existing_fm.source_skill_id,
        skill_note_id,
        contract.input_kind.as_deref(),
    )
    .await;
    fm.source_skill_id = Some(resolved_source_skill_id);
    fm.status = match contract.gate {
        crate::plugins::skill::frontmatter::SkillGate::Auto => {
            ArtifactStatus::Approved
        }
        crate::plugins::skill::frontmatter::SkillGate::Approval => {
            ArtifactStatus::Pending
        }
    };
    let final_body = rewrite_artifact_fm(&new_body, &fm);
    if let Err(e) = persistence
        .save(&source_note_id.to_string(), final_body.as_bytes())
        .await
    {
        tracing::warn!(
            target: "operon::artifact",
            "normalizer rewrite save failed for {source_note_id}: {e}"
        );
        return Vec::new();
    }
    // Best-effort: delete the scratch file so a subsequent
    // `scan_produced_artifact_files` doesn't import it again on a later run.
    let _ = std::fs::remove_file(first);
    vec![source_note_id]
}

/// Decide which `source_skill_id` to stamp on an artifact a normalizer
/// just rewrote. Loads the prior pointer's skill contract from
/// persistence and delegates the actual decision to the pure
/// `decide_normalizer_source_skill_id` so the rule stays
/// unit-testable. Falls back to "stamp this normalizer" on any I/O
/// or parse failure — the conservative default keeps the original
/// behaviour for hand-authored artifacts that have no prior
/// producer.
async fn resolve_normalizer_source_skill_id(
    persistence: &Arc<dyn Persistence>,
    existing_source_skill_id: Option<Uuid>,
    this_normalizer_id: Uuid,
    this_normalizer_input_kind: Option<&str>,
) -> Uuid {
    let Some(prior) = existing_source_skill_id else {
        return this_normalizer_id;
    };
    if prior == this_normalizer_id {
        return this_normalizer_id;
    }
    let producer_contract = load_skill_contract(persistence, prior).await;
    decide_normalizer_source_skill_id(
        existing_source_skill_id,
        this_normalizer_id,
        producer_contract.as_ref(),
        this_normalizer_input_kind,
    )
}

/// Pure decision: given the artifact's prior `source_skill_id`, this
/// normalizer's id, the prior producer's contract (if loadable), and
/// this normalizer's `input_kind`, return the id to stamp.
///
/// Preserves the prior pointer when it resolves to a real upstream
/// producer (a non-normalizer skill whose `output_kind` matches this
/// normalizer's `input_kind`). Otherwise stamps this normalizer.
///
/// Why preserve: the cascade pipes a freshly-produced artifact
/// straight into its sibling normalizer (e.g. an Epic from
/// `02-discover-epics` into `02n-normalize-epics`). If the
/// normalizer overwrites `source_skill_id` to its own id, the next
/// cascade pass calls `existing_children_with_skill(parent, 02)` and
/// fails to recognize the artifact as already produced by `02` —
/// the parent's skip-already-produced gate then orphans the
/// artifact from cascade traversal, and once every sibling has been
/// orphaned the gate falls through and re-fires `02`, regenerating
/// duplicates.
fn decide_normalizer_source_skill_id(
    existing_source_skill_id: Option<Uuid>,
    this_normalizer_id: Uuid,
    producer_contract: Option<&crate::plugins::skill::frontmatter::SkillContract>,
    this_normalizer_input_kind: Option<&str>,
) -> Uuid {
    let Some(prior) = existing_source_skill_id else {
        return this_normalizer_id;
    };
    if prior == this_normalizer_id {
        return this_normalizer_id;
    }
    let Some(producer) = producer_contract else {
        return this_normalizer_id;
    };
    // Reject prior pointers that are themselves normalizers
    // (input_kind == output_kind). We only want to preserve real
    // upstream producers — a chain of normalizers all sharing the
    // same kind has no "true" producer to keep, so just stamp this
    // normalizer.
    if producer
        .input_kind
        .as_deref()
        .zip(producer.output_kind.as_deref())
        .map(|(i, o)| i == o)
        .unwrap_or(false)
    {
        return this_normalizer_id;
    }
    let Some(this_input) = this_normalizer_input_kind else {
        return this_normalizer_id;
    };
    if matches!(producer.output_kind.as_deref(), Some(k) if k == this_input) {
        prior
    } else {
        this_normalizer_id
    }
}

/// Load a skill note's parsed contract from persistence. Returns
/// `None` on any I/O / utf8 / frontmatter-split failure so callers
/// can degrade gracefully rather than panic.
async fn load_skill_contract(
    persistence: &Arc<dyn Persistence>,
    skill_id: Uuid,
) -> Option<crate::plugins::skill::frontmatter::SkillContract> {
    let bytes = persistence.load(&skill_id.to_string()).await.ok()?;
    let body = String::from_utf8(bytes).ok()?;
    let (lines, _) = crate::plugins::skill::frontmatter::split(&body);
    let lines = lines?;
    Some(crate::plugins::skill::frontmatter::contract(&lines))
}

/// Clear the `revision_notes` field on an artifact whose body we
/// already have in memory (the source artifact's body was loaded at
/// the start of the run). Saves the rewritten body back to
/// persistence. Failures are logged and ignored — clearing notes is
/// best-effort cleanup, never load-bearing for the run's success.
async fn clear_revision_notes(
    persistence: &Arc<dyn Persistence>,
    artifact_id: Uuid,
    body: &str,
) {
    let mut fm = crate::plugins::artifact::frontmatter::parse(body);
    if fm.revision_notes.is_none() {
        return;
    }
    fm.revision_notes = None;
    let rewritten = rewrite_artifact_fm(body, &fm);
    if let Err(e) = persistence
        .save(&artifact_id.to_string(), rewritten.as_bytes())
        .await
    {
        tracing::warn!(
            target: "operon::artifact",
            "clear_revision_notes save failed for {artifact_id}: {e}"
        );
    }
}

/// Clear an artifact's `revision_notes` field by id when the caller
/// doesn't already have its body in memory (the Re-run path uses this
/// for the dirty *output* artifact). Loads body, rewrites, saves.
async fn clear_revision_notes_by_id(
    persistence: &Arc<dyn Persistence>,
    artifact_id: Uuid,
) {
    let bytes = match persistence.load(&artifact_id.to_string()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "clear_revision_notes_by_id load failed for {artifact_id}: {e}"
            );
            return;
        }
    };
    let body = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "operon::artifact",
                "clear_revision_notes_by_id utf8 for {artifact_id}: {e}"
            );
            return;
        }
    };
    clear_revision_notes(persistence, artifact_id, &body).await;
}

/// `true` for file extensions the artifact importer recognises.
/// `.md` is the historic format; `.mdx` was added so skills 02–09 can
/// emit mid-run `clarification` artifacts whose extension makes the
/// origin (inline-clarification flow vs. `00-coherence-check`) visible
/// at a glance in the file tree. Both extensions parse identically.
fn is_artifact_file_extension(ext: Option<&str>) -> bool {
    matches!(ext, Some("md") | Some("mdx"))
}

/// List artifact-source files (top-level only — no recursion) in
/// `dir`. Accepts `.md` and `.mdx`. Returns absolute, canonicalized
/// paths so the diff against post-run state works regardless of how
/// `dir` was originally constructed.
fn list_artifact_files(dir: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && is_artifact_file_extension(path.extension().and_then(|e| e.to_str()))
        {
            if let Ok(canon) = path.canonicalize() {
                out.insert(canon);
            } else {
                out.insert(path);
            }
        }
    }
    out
}

/// Find artifact-source files (`.md` or `.mdx`) in `dir` that are
/// either NEW (not in `pre_existing`) or were modified after
/// `run_started_at` (the latter handles re-runs that overwrite a
/// prior file). Returned in lexicographic order so imports are
/// deterministic across runs.
fn scan_produced_artifact_files(
    dir: &Path,
    pre_existing: &HashSet<PathBuf>,
    run_started_at: SystemTime,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !(path.is_file()
            && is_artifact_file_extension(path.extension().and_then(|e| e.to_str())))
        {
            continue;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let is_new = !pre_existing.contains(&canonical);
        let is_recent = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| t >= run_started_at)
            .unwrap_or(false);
        if is_new || is_recent {
            out.push(canonical);
        }
    }
    out.sort();
    out
}

/// Mirror the workflow executor's chat-message-persist contract so
/// the rail entry reads the same as a regular companion chat:
/// User → Assistant → ToolCall → ToolResult flow. Text deltas are
/// buffered into a single Assistant row per turn (caller flushes
/// after the loop ends). Returns `true` when this call resulted in
/// at least one row mutation (append or update_tool_result), so the
/// caller knows to bump the live-transcript version signal.
fn persist_event(
    repo: &Arc<dyn ChatMessageRepository>,
    chat_session_id: Uuid,
    ev: &ClaudeCodeEvent,
    assistant_buf: &mut String,
) -> bool {
    use ClaudeCodeEvent::*;
    let flush = |buf: &mut String| -> bool {
        if buf.is_empty() {
            // Still clear the in-progress entry so the streaming
            // block disappears even when there's nothing to flush.
            crate::shell::companion_state::INPROGRESS_ASSISTANT.with_mut(|m| {
                m.remove(&chat_session_id);
            });
            return false;
        }
        // Body shape MUST be `{ "body": "<text>" }` to match the
        // companion's transcript_item_from_message Assistant case.
        let _ = repo.append(
            chat_session_id,
            ChatMessageKind::Assistant,
            None,
            &serde_json::json!({ "body": std::mem::take(buf) }),
        );
        // Clear the streaming entry — the persisted row is now the
        // canonical surface for this assistant block.
        crate::shell::companion_state::INPROGRESS_ASSISTANT.with_mut(|m| {
            m.remove(&chat_session_id);
        });
        true
    };
    match ev {
        Text(t) => {
            assistant_buf.push_str(t);
            // Live-stream this delta into the in-progress map. The
            // companion's render reads `INPROGRESS_ASSISTANT` and
            // renders the entry as a transient block at the bottom
            // of the transcript with a blinking cursor — letter-
            // by-letter streaming UX without DB churn.
            crate::shell::companion_state::INPROGRESS_ASSISTANT.with_mut(|m| {
                m.entry(chat_session_id).or_default().push_str(t);
            });
            // Returning false so the outer loop doesn't bump
            // CHAT_MESSAGE_VERSION (no chat_message row yet); the
            // GlobalSignal write above re-renders the streaming
            // surface directly.
            false
        }
        Thinking(t) => {
            let flushed = flush(assistant_buf);
            let _ = repo.append(
                chat_session_id,
                ChatMessageKind::Thinking,
                None,
                &serde_json::json!({ "text": t }),
            );
            flushed || true
        }
        ToolUse { id, name, input } => {
            let flushed = flush(assistant_buf);
            let _ = repo.append(
                chat_session_id,
                ChatMessageKind::ToolCall,
                Some(id),
                &serde_json::json!({
                    "id": id,
                    "name": name,
                    "input": input,
                    "result": serde_json::Value::Null,
                }),
            );
            // Phase F: when claude uses the Write tool to produce an
            // artifact file, mirror the file's content into the rail
            // as a readable Assistant message alongside the
            // (collapsible) ToolCall card. The card has the
            // structural details (path, status); the Assistant
            // block has the markdown body so the user can follow
            // each artifact's content as a streaming text feed
            // rather than digging into the JSON-formatted ToolCall
            // input.
            if name == "Write" {
                if let Some(content) = input.get("content").and_then(|v| v.as_str()) {
                    let path = input
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("artifact");
                    let label = std::path::Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(path);
                    let body = format!("\u{1F4C4} **{label}**\n\n{content}");
                    let _ = repo.append(
                        chat_session_id,
                        ChatMessageKind::Assistant,
                        None,
                        &serde_json::json!({ "body": body }),
                    );
                }
            }
            flushed || true
        }
        ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let _ = repo.update_tool_result(
                chat_session_id,
                tool_use_id,
                &serde_json::json!({
                    "id": tool_use_id,
                    "result": { "content": content, "is_error": is_error },
                }),
            );
            true
        }
        Done { .. } | Error(_) => flush(assistant_buf),
        // Init carries MCP roster + tool inventory but the artifact
        // runner doesn't render those — `apply_event` in
        // `companion_chat.rs` is the one place that mirrors them into
        // `MCP_LIVE_STATUS`. Drop here.
        SessionInit { .. } => false,
    }
}

/// Bump the global live-transcript version so the companion's
/// poll loop re-fetches `chat_message`. Uses the application-wide
/// `CHAT_MESSAGE_VERSION` `GlobalSignal` rather than a
/// context-provided `Signal` — the runner's task lives in the
/// virtual root scope (via `spawn_forever`), and writes from there
/// to a scope-bound signal are silently dropped (Dioxus emits a
/// `__copy_value_hoisted` warning).
fn bump_message_version() {
    crate::shell::companion_state::CHAT_MESSAGE_VERSION.with_mut(|v| {
        *v = v.saturating_add(1);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_artifact_files_returns_md_and_mdx() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "x").unwrap();
        std::fs::write(dir.path().join("clarification-01-foo.mdx"), "z").unwrap();
        std::fs::write(dir.path().join("b.txt"), "y").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        let set = list_artifact_files(dir.path());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn scan_produced_artifact_files_diffs_against_pre_existing() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.md");
        std::fs::write(&stale, "x").unwrap();
        let pre = list_artifact_files(dir.path());
        let started = SystemTime::now();
        // Sleep so the new files' mtimes are provably after `started`.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let fresh = dir.path().join("fresh.md");
        std::fs::write(&fresh, "y").unwrap();
        let clarification = dir.path().join("clarification-01-tenancy.mdx");
        std::fs::write(&clarification, "z").unwrap();
        let produced = scan_produced_artifact_files(dir.path(), &pre, started);
        assert_eq!(produced.len(), 2);
        let names: Vec<&str> = produced
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(names.contains(&"fresh.md"));
        assert!(names.contains(&"clarification-01-tenancy.mdx"));
    }

    #[test]
    fn build_prompt_inlines_source_and_skill_and_dir() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        let prompt = build_prompt(
            "REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &[],
            None,
            &[],
            &[],
        );
        assert!(prompt.contains("REQ_BODY"));
        assert!(prompt.contains("SKILL_BODY"));
        assert!(prompt.contains("/tmp/x"));
        assert!(prompt.contains("artifact_kind: epic"));
        // No aggregate / inherit / previous-revisions sections when none provided.
        assert!(!prompt.contains("aggregated"));
        assert!(!prompt.contains("inherited"));
        assert!(!prompt.contains("previous revisions to preserve"));
        // Phase D: bundle section also absent when no entries.
        assert!(!prompt.contains("master-requirement subtree"));
    }

    #[test]
    fn build_prompt_inlines_master_req_subtree_bundle() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        let bundle = vec![
            BundleEntry::Text {
                title: "user-personas".into(),
                body: "Persona A: power user".into(),
            },
            BundleEntry::Image {
                title: "ui-mockup".into(),
                path: std::path::PathBuf::from("/vault/.operon/images/mockup.png"),
            },
        ];
        let prompt = build_prompt(
            "MASTER_REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &[],
            None,
            &[],
            &bundle,
        );
        assert!(prompt.contains("master-requirement subtree (1 text + 1 image)"));
        assert!(prompt.contains("--- subtree: user-personas ---"));
        assert!(prompt.contains("Persona A: power user"));
        assert!(prompt.contains("--- subtree image: ui-mockup ---"));
        assert!(prompt.contains("/vault/.operon/images/mockup.png"));
    }

    #[test]
    fn parse_priority_order_reads_unordered_bullets() {
        let body =
            "# Backlog\n\n## Priority order\n- T001\n- T003 (was bumped past T002)\n- T002\n\n## Risks\n- none\n";
        assert_eq!(parse_priority_order(body), vec!["T001", "T003", "T002"]);
    }

    #[test]
    fn parse_priority_order_reads_ordered_bullets() {
        let body =
            "## Priority order\n1. task-01-add-user-table\n2. task-02-login-form\n10. task-10-cleanup\n";
        assert_eq!(
            parse_priority_order(body),
            vec![
                "task-01-add-user-table",
                "task-02-login-form",
                "task-10-cleanup",
            ]
        );
    }

    #[test]
    fn parse_priority_order_skips_none_marker_and_prose() {
        let body =
            "## Priority order\n\nSome rationale prose.\n- None\n- T001\n";
        assert_eq!(parse_priority_order(body), vec!["T001"]);
    }

    #[test]
    fn parse_priority_order_returns_empty_when_section_absent() {
        let body = "# Backlog\n\n## Notes\n- nothing\n";
        assert!(parse_priority_order(body).is_empty());
    }

    #[test]
    fn build_prompt_inlines_aggregated_artifacts() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("prioritized_backlog".into());
        contract.aggregate = Some("task".into());
        let aggregated = vec![
            ("task-01-add-user-table".into(), "Body of task 1".into()),
            ("task-02-add-login-form".into(), "Body of task 2".into()),
        ];
        let prompt = build_prompt(
            "SEED_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &aggregated,
            &[],
            None,
            &[],
            &[],
        );
        assert!(prompt.contains("SEED_BODY"));
        assert!(prompt.contains("aggregated task artifacts (2 total)"));
        assert!(prompt.contains("--- artifact: task-01-add-user-table ---"));
        assert!(prompt.contains("Body of task 1"));
        assert!(prompt.contains("--- artifact: task-02-add-login-form ---"));
        assert!(prompt.contains("Body of task 2"));
    }

    #[test]
    fn build_prompt_inlines_inherited_artifacts() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("implementation".into());
        contract.inherit = Some("plan".into());
        let inherited = vec![
            ("plan-hld-feature-auth".into(), "HLD plan body".into()),
            ("plan-lld-story-login".into(), "LLD plan body".into()),
        ];
        let prompt = build_prompt(
            "TASK_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &inherited,
            None,
            &[],
            &[],
        );
        assert!(prompt.contains("TASK_BODY"));
        assert!(prompt.contains(
            "inherited plan artifacts from ancestor chain (2 total)"
        ));
        assert!(prompt.contains("--- artifact: plan-hld-feature-auth ---"));
        assert!(prompt.contains("HLD plan body"));
        assert!(prompt.contains("--- artifact: plan-lld-story-login ---"));
        assert!(prompt.contains("LLD plan body"));
        // No aggregate section when only inherit is set.
        assert!(!prompt.contains("aggregated"));
    }

    #[test]
    fn build_prompt_inlines_revision_notes_when_present() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        let prompt = build_prompt(
            "REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &[],
            Some("Emphasize observability concerns"),
            &[],
            &[],
        );
        assert!(prompt.contains("--- refinement notes from user ---"));
        assert!(prompt.contains("Emphasize observability concerns"));
        assert!(prompt.contains("--- /refinement notes from user ---"));
    }

    #[test]
    fn build_prompt_skips_revision_notes_when_blank() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        // Whitespace-only notes must not produce an empty fence.
        let prompt = build_prompt(
            "REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &[],
            Some("   \n  "),
            &[],
            &[],
        );
        assert!(!prompt.contains("refinement notes"));
    }

    #[test]
    fn build_prompt_inlines_previous_outputs_when_present() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        let previous = vec![
            ("epic-01-onboarding".into(), "Body of the prior onboarding epic".into()),
            ("epic-02-billing".into(), "Body of the prior billing epic".into()),
        ];
        let prompt = build_prompt(
            "REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &[],
            None,
            &previous,
            &[],
        );
        assert!(prompt.contains("previous revisions to preserve (2 total)"));
        assert!(prompt.contains("--- previous: epic-01-onboarding ---"));
        assert!(prompt.contains("Body of the prior onboarding epic"));
        assert!(prompt.contains("--- previous: epic-02-billing ---"));
        assert!(prompt.contains("Body of the prior billing epic"));
        assert!(prompt.contains("--- /previous revisions to preserve ---"));
    }

    #[test]
    fn build_prompt_skips_previous_outputs_section_when_empty() {
        let dir = std::path::Path::new("/tmp/x");
        let mut contract = crate::plugins::skill::frontmatter::SkillContract::default();
        contract.output_kind = Some("epic".into());
        let prompt = build_prompt(
            "REQ_BODY",
            "SKILL_BODY",
            dir,
            &contract,
            Uuid::nil(),
            Uuid::nil(),
            &[],
            &[],
            None,
            &[],
            &[],
        );
        assert!(!prompt.contains("previous revisions to preserve"));
    }

    #[test]
    fn combine_revision_notes_merges_source_and_extra() {
        let merged = combine_revision_notes(
            Some("from source"),
            Some(&(Uuid::nil(), "from output".to_string())),
        )
        .expect("combined notes");
        assert!(merged.contains("[from source artifact]"));
        assert!(merged.contains("from source"));
        assert!(merged.contains("[from output artifact]"));
        assert!(merged.contains("from output"));
    }

    #[test]
    fn combine_revision_notes_returns_none_when_both_blank() {
        assert!(combine_revision_notes(None, None).is_none());
        assert!(combine_revision_notes(Some("   "), None).is_none());
        assert!(
            combine_revision_notes(None, Some(&(Uuid::nil(), "  ".into()))).is_none()
        );
    }

    #[test]
    fn is_normalizer_contract_recognizes_matching_kinds() {
        let mut c = crate::plugins::skill::frontmatter::SkillContract::default();
        c.input_kind = Some("epic".into());
        c.output_kind = Some("epic".into());
        c.output_count = crate::plugins::skill::frontmatter::SkillOutputCount::One;
        assert!(is_normalizer_contract(&c));
    }

    #[test]
    fn is_normalizer_contract_rejects_mismatched_kinds() {
        let mut c = crate::plugins::skill::frontmatter::SkillContract::default();
        c.input_kind = Some("epic".into());
        c.output_kind = Some("feature".into());
        c.output_count = crate::plugins::skill::frontmatter::SkillOutputCount::One;
        assert!(!is_normalizer_contract(&c));
    }

    #[test]
    fn is_normalizer_contract_rejects_many_output_count() {
        let mut c = crate::plugins::skill::frontmatter::SkillContract::default();
        c.input_kind = Some("epic".into());
        c.output_kind = Some("epic".into());
        c.output_count = crate::plugins::skill::frontmatter::SkillOutputCount::Many;
        assert!(!is_normalizer_contract(&c));
    }

    #[test]
    fn is_normalizer_contract_rejects_missing_kinds() {
        let c = crate::plugins::skill::frontmatter::SkillContract::default();
        assert!(!is_normalizer_contract(&c));
    }

    fn make_producer_contract(input: &str, output: &str) -> crate::plugins::skill::frontmatter::SkillContract {
        let mut c = crate::plugins::skill::frontmatter::SkillContract::default();
        c.input_kind = Some(input.into());
        c.output_kind = Some(output.into());
        c
    }

    #[test]
    fn normalizer_source_skill_preserves_real_upstream_producer() {
        // 02 (master_requirement → epic) produced this artifact;
        // 02n (epic → epic) is now rewriting it. Preserve 02 so the
        // parent's skip-already-produced gate keeps recognising the
        // artifact as a child of 02.
        let producer_id = Uuid::new_v4();
        let normalizer_id = Uuid::new_v4();
        let producer = make_producer_contract("master_requirement", "epic");
        let resolved = decide_normalizer_source_skill_id(
            Some(producer_id),
            normalizer_id,
            Some(&producer),
            Some("epic"),
        );
        assert_eq!(resolved, producer_id);
    }

    #[test]
    fn normalizer_source_skill_stamps_self_when_no_prior_producer() {
        // Hand-authored artifact with no source_skill_id — the
        // normalizer is the only producer pointer we can offer, so
        // stamp it.
        let normalizer_id = Uuid::new_v4();
        let resolved = decide_normalizer_source_skill_id(
            None,
            normalizer_id,
            None,
            Some("epic"),
        );
        assert_eq!(resolved, normalizer_id);
    }

    #[test]
    fn normalizer_source_skill_idempotent_on_self() {
        // Re-running the same normalizer on its own output: the prior
        // pointer already points at us. Stamp ourselves (a no-op
        // semantically).
        let normalizer_id = Uuid::new_v4();
        let resolved = decide_normalizer_source_skill_id(
            Some(normalizer_id),
            normalizer_id,
            None,
            Some("epic"),
        );
        assert_eq!(resolved, normalizer_id);
    }

    #[test]
    fn normalizer_source_skill_stamps_self_when_producer_is_normalizer() {
        // The prior pointer is itself a normalizer (input == output).
        // No real producer to preserve, so stamp this normalizer.
        let prior_normalizer_id = Uuid::new_v4();
        let this_normalizer_id = Uuid::new_v4();
        let prior = make_producer_contract("epic", "epic");
        let resolved = decide_normalizer_source_skill_id(
            Some(prior_normalizer_id),
            this_normalizer_id,
            Some(&prior),
            Some("epic"),
        );
        assert_eq!(resolved, this_normalizer_id);
    }

    #[test]
    fn normalizer_source_skill_stamps_self_when_producer_kind_misaligned() {
        // Prior pointer's output_kind doesn't match this normalizer's
        // input_kind (e.g. an artifact that was copied across kinds).
        // Stamp this normalizer rather than preserve a stale link.
        let producer_id = Uuid::new_v4();
        let normalizer_id = Uuid::new_v4();
        let producer = make_producer_contract("master_requirement", "feature");
        let resolved = decide_normalizer_source_skill_id(
            Some(producer_id),
            normalizer_id,
            Some(&producer),
            Some("epic"),
        );
        assert_eq!(resolved, normalizer_id);
    }

    #[test]
    fn decide_normalizer_skip_skips_when_source_skill_id_set() {
        // Any prior producer pointer means the body is canonical;
        // re-running the normalizer would only append a duplicate
        // revision row.
        assert!(decide_normalizer_skip(Some(Uuid::new_v4())));
    }

    #[test]
    fn decide_normalizer_skip_runs_when_source_skill_id_absent() {
        // Hand-authored artifact (no upstream producer) — let the
        // normalizer canonicalise the body once.
        assert!(!decide_normalizer_skip(None));
    }

    #[test]
    fn decide_normalizer_skip_treats_self_as_already_done() {
        // The gate doesn't care which skill produced the artifact —
        // any `Some(_)` short-circuits, including the case where the
        // pointer happens to be the normalizer itself (artifact was
        // hand-authored, then normalized once before).
        let normalizer_id = Uuid::new_v4();
        assert!(decide_normalizer_skip(Some(normalizer_id)));
    }

    #[test]
    fn normalizer_source_skill_stamps_self_when_producer_unloadable() {
        // Producer skill couldn't be loaded (deleted, IO error, etc.).
        // Degrade to "stamp this normalizer" rather than preserve a
        // dangling pointer.
        let producer_id = Uuid::new_v4();
        let normalizer_id = Uuid::new_v4();
        let resolved = decide_normalizer_source_skill_id(
            Some(producer_id),
            normalizer_id,
            None,
            Some("epic"),
        );
        assert_eq!(resolved, normalizer_id);
    }
}
