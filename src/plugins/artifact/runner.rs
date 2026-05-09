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
use crate::plugins::artifact::frontmatter::{
    parse as parse_artifact_fm, rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
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
    let source_body =
        String::from_utf8(source_bytes).map_err(|e| RunnerError::Plugin(format!("utf8: {e}")))?;

    // 2a. Pipeline gate: refuse runs when the source is not Approved
    //     unless it's a root seed (no upstream parent — e.g. a
    //     user-authored Requirements note). Mirrors the UI-side gate
    //     in `src/plugins/artifact/view.rs`. Skip the check entirely
    //     when the source isn't even an Artifact-frontmatter note —
    //     the workflow canvas reuses some of these paths for plain
    //     Markdown sources.
    let source_fm = crate::plugins::artifact::frontmatter::parse(&source_body);
    if source_fm.artifact_kind.is_some()
        && source_fm.source_artifact_id.is_some()
        && source_fm.status != ArtifactStatus::Approved
    {
        let path_label =
            build_artifact_path_label(note_repo, project_repo, project_id, source_note_id);
        return Err(RunnerError::Gated(format!(
            "source artifact \"{path_label}\" is {} — approve it before running downstream skills",
            source_fm.status.as_str()
        )));
    }

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

    // 4. Where claude is going to write the new artifact files. One
    //    subdir per source so the engine can scan a known place after
    //    the run completes.
    let artifacts_dir = repo_path
        .join(".operon")
        .join("artifacts")
        .join(source_note_id.to_string());
    std::fs::create_dir_all(&artifacts_dir)?;
    let run_started_at = SystemTime::now();

    // 5. Pre-snapshot the directory so we only import files claude
    //    creates *during this run*, not pre-existing leftovers from a
    //    prior run that the user already imported.
    let existing: HashSet<PathBuf> = list_md_files(&artifacts_dir);

    // 6. Build the prompt that claude will see.
    let prompt = build_prompt(
        &source_body,
        &skill_body,
        &artifacts_dir,
        &contract,
        source_note_id,
        skill_note_id,
        &aggregated,
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

    // 8. Run claude. The runner forces `acceptEdits` on this
    //    session so its automated Write tool calls don't hang
    //    waiting for stdin approval — even when the user's global
    //    permission picker is set to "default". Normal companion
    //    chats keep using whatever the user picked, since they
    //    don't set a per-session override.
    plugin.set_session_permission_mode(chat_session_id, Some("acceptEdits".into()));
    let ct = CancellationToken::new();
    let mut rx = plugin
        .send_rich(prompt, chat_session_id, ct)
        .await
        .map_err(|e| RunnerError::Plugin(format!("send_rich: {e}")))?;
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
        match ev {
            ClaudeCodeEvent::Done { .. } => break,
            ClaudeCodeEvent::Error(msg) => {
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
    let produced = scan_produced_files(&artifacts_dir, &existing, run_started_at);

    // 10. Import each produced file as an Artifact note under the
    //     source. Body is read from disk; frontmatter is patched so
    //     the engine's view fields (status, source linkage) are
    //     authoritative regardless of what claude wrote.
    //
    //     Dedup: if a sibling Artifact note with the same title
    //     already exists under this source, reuse its row id and
    //     overwrite the body. Without this, every Re-run / Revise
    //     cycle would duplicate every child artifact under the same
    //     parent — making the explorer tree increasingly noisy and
    //     making "regenerate after editing the Epic" a destructive
    //     UX (the user would have to manually delete N stale rows).
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
        let body = match std::fs::read_to_string(&file) {
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
        // Patch the artifact frontmatter so source_artifact_id /
        // source_skill_id / status are always correct, even if the
        // skill prompt forgot to emit them.
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

    // 11. Workflow emission: prioritization skills declare
    //     `emit_workflow: true` so the runner reads the produced
    //     backlog artifact's `## Priority order` section, looks up
    //     each named task in the project, and writes a sibling
    //     `NoteKind::Workflow` note with one snapshot node per
    //     prioritized task plus depends-on cross-edges parsed from
    //     each task's body. The Workflow note opens to the existing
    //     React Flow canvas so the user gets the cross-story DAG
    //     they asked for without any new view code.
    if contract.emit_workflow {
        for backlog_id in &created_ids {
            if let Err(e) = emit_workflow_for_backlog(
                note_repo,
                persistence,
                project_id,
                source_note_id,
                *backlog_id,
            )
            .await
            {
                tracing::warn!(
                    target: "operon::artifact",
                    "emit_workflow for backlog {backlog_id} failed: {e}"
                );
            }
        }
    }

    Ok(RunOutcome {
        created_artifact_ids: created_ids,
        artifacts_dir,
    })
}

/// Build (or refresh) the sibling Workflow note for a prioritized
/// backlog artifact. Reads the backlog's `## Priority order` to get
/// task titles in priority order, resolves each to an Artifact note
/// id under the seed, snapshots them as nodes (re-using the
/// `is_artifact_snapshot` shape established by `cascade_graph`), and
/// adds depends-on edges parsed from each task body's `## Depends on`
/// section. Idempotent on re-runs — same title resolves to the same
/// Workflow note and the body is overwritten.
async fn emit_workflow_for_backlog(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
    backlog_id: Uuid,
) -> Result<(), String> {
    let backlog_bytes = persistence
        .load(&backlog_id.to_string())
        .await
        .map_err(|e| format!("load backlog: {e}"))?;
    let backlog_body =
        String::from_utf8(backlog_bytes).map_err(|e| format!("backlog utf8: {e}"))?;
    let priority = parse_priority_order(&backlog_body);
    if priority.is_empty() {
        return Err("backlog body has no `## Priority order` entries".into());
    }

    // Index every artifact in the project by title (and TaskID
    // prefix) so a `T001` or `task-01-foo` line resolves cheaply.
    let all_notes = note_repo
        .list_for_project(project_id)
        .map_err(|e| format!("list_for_project: {e}"))?;
    let mut by_title: std::collections::HashMap<String, &LocalNote> =
        std::collections::HashMap::new();
    for n in &all_notes {
        if !matches!(n.kind, NoteKind::Artifact) {
            continue;
        }
        by_title.insert(n.title.clone(), n);
        if let Some(first) = n.title.split_whitespace().next() {
            by_title.insert(first.to_string(), n);
        }
    }

    // Resolve priority slugs to Artifact notes; drop unresolved.
    let mut resolved: Vec<&LocalNote> = Vec::new();
    for slug in &priority {
        if let Some(n) = by_title.get(slug.as_str()) {
            resolved.push(*n);
        }
    }
    if resolved.is_empty() {
        return Err("no priority entries resolved to known artifacts".into());
    }

    // Build (or reuse) the sibling Workflow note. Title is derived
    // from the backlog artifact so coarse + refined runs each get
    // their own canvas without colliding.
    let backlog_title = all_notes
        .iter()
        .find(|n| n.id == backlog_id)
        .map(|n| n.title.clone())
        .unwrap_or_else(|| "backlog".to_string());
    let workflow_title = format!("Workflow — {backlog_title}");
    let existing = all_notes
        .iter()
        .find(|n| {
            matches!(n.kind, NoteKind::Workflow)
                && n.parent_id == Some(seed_id)
                && n.title == workflow_title
        })
        .map(|n| n.id);
    let workflow_id = match existing {
        Some(id) => id,
        None => note_repo
            .create_with_kind(
                project_id,
                Some(seed_id),
                &workflow_title,
                NoteKind::Workflow,
            )
            .map_err(|e| format!("create_with_kind workflow: {e}"))?
            .id,
    };

    // Build the graph. Snapshot nodes (`is_artifact_snapshot:true`)
    // reuse the read-only render path the cascade graph already uses.
    // Layout: priority order along the X axis at a single Y so the
    // canvas opens to a clear left-to-right backlog stripe; depends-
    // on edges become amber arrows above the stripe.
    let mut graph = WorkflowGraph::new();
    let mut node_id_by_artifact: std::collections::HashMap<Uuid, Uuid> =
        std::collections::HashMap::new();
    let mut bodies_by_artifact: std::collections::HashMap<Uuid, String> =
        std::collections::HashMap::new();
    for (i, art) in resolved.iter().enumerate() {
        let body = persistence
            .load(&art.id.to_string())
            .await
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let kind_label = parse_artifact_fm(&body)
            .artifact_kind
            .as_ref()
            .map(|k| k.display_name())
            .unwrap_or_else(|| "Artifact".into());
        let nid = Uuid::new_v4();
        graph.nodes.insert(
            nid,
            Node {
                id: nid,
                skill_note_id: Uuid::nil(),
                typed_fields: serde_json::Value::Null,
                extra_instructions: String::new(),
                position: ((i as f64) * 200.0, 80.0),
                cached_output_path: None,
                cached_input_hash: None,
                status: NodeStatus::Fresh,
                cached_output_note_id: None,
                is_artifact_snapshot: true,
                artifact_ref: Some(art.id),
                artifact_kind_label: Some(kind_label),
                artifact_title: Some(art.title.clone()),
            },
        );
        node_id_by_artifact.insert(art.id, nid);
        bodies_by_artifact.insert(art.id, body);
    }

    // Depends-on edges, reusing the same parser the cascade graph
    // uses for sibling cross-edges. References that don't resolve to
    // a node in this backlog are silently dropped.
    for (artifact_id, body) in &bodies_by_artifact {
        let to = match node_id_by_artifact.get(artifact_id) {
            Some(n) => *n,
            None => continue,
        };
        for slug in crate::plugins::artifact::cascade_graph::parse_depends_on(body) {
            let from_artifact = match by_title.get(slug.as_str()) {
                Some(n) => n.id,
                None => continue,
            };
            let from = match node_id_by_artifact.get(&from_artifact) {
                Some(n) => *n,
                None => continue,
            };
            if from == to {
                continue;
            }
            let dup = graph.edges.iter().any(|e| {
                e.from == from
                    && e.to == to
                    && e.edge_kind.as_deref() == Some("depends_on")
            });
            if !dup {
                graph.edges.push(Edge {
                    id: Uuid::new_v4(),
                    from,
                    from_socket: "default".into(),
                    to,
                    to_socket: "default".into(),
                    edge_kind: Some("depends_on".into()),
                });
            }
        }
    }
    graph.version = graph.version.saturating_add(1);

    let body = serde_json::to_string_pretty(&graph)
        .map_err(|e| format!("serialize graph: {e}"))?;
    persistence
        .save(&workflow_id.to_string(), body.as_bytes())
        .await
        .map_err(|e| format!("save workflow note: {e}"))?;
    Ok(())
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

fn build_prompt(
    source_body: &str,
    skill_body: &str,
    artifacts_dir: &Path,
    contract: &crate::plugins::skill::frontmatter::SkillContract,
    source_id: Uuid,
    skill_id: Uuid,
    aggregated: &[(String, String)],
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

    buf.push_str(
        "When done, do NOT echo the artifact contents back to the user — the\n\
         engine reads them from disk. A short summary of how many artifacts\n\
         you produced is enough.\n",
    );
    buf
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

/// Aggregator helper: walk the descendants of `seed_id` under
/// `project_id` and return `(title, body)` for every Artifact note
/// whose `artifact_kind` matches `wanted_kind`. BFS, ordered by note
/// title so the prompt is deterministic across runs. Skips the seed
/// itself even if it happens to be the same kind (the seed body is
/// already inlined separately).
async fn collect_descendant_artifacts(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
    wanted_kind: &str,
) -> Vec<(String, String)> {
    let all = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut by_parent: std::collections::HashMap<Uuid, Vec<&LocalNote>> =
        std::collections::HashMap::new();
    for n in &all {
        if let Some(p) = n.parent_id {
            by_parent.entry(p).or_default().push(n);
        }
    }
    let mut visited = HashSet::new();
    let mut queue: std::collections::VecDeque<Uuid> = std::collections::VecDeque::new();
    queue.push_back(seed_id);
    visited.insert(seed_id);
    let mut matched: Vec<&LocalNote> = Vec::new();
    while let Some(id) = queue.pop_front() {
        if let Some(children) = by_parent.get(&id) {
            for child in children {
                if !visited.insert(child.id) {
                    continue;
                }
                if matches!(child.kind, NoteKind::Artifact) {
                    matched.push(child);
                }
                queue.push_back(child.id);
            }
        }
    }
    matched.sort_by(|a, b| a.title.cmp(&b.title));
    let mut out: Vec<(String, String)> = Vec::with_capacity(matched.len());
    for n in matched {
        let bytes = match persistence.load(&n.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fm = parse_artifact_fm(&body);
        let matches_kind = fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == wanted_kind)
            .unwrap_or(false);
        if !matches_kind {
            continue;
        }
        out.push((n.title.clone(), body));
    }
    out
}

/// List `.md` files (top-level only — no recursion) in `dir`.
/// Returns absolute, canonicalized paths so the diff against post-run
/// state works regardless of how `dir` was originally constructed.
fn list_md_files(dir: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(canon) = path.canonicalize() {
                out.insert(canon);
            } else {
                out.insert(path);
            }
        }
    }
    out
}

/// Find files in `dir` that are either NEW (not in `pre_existing`) or
/// were modified after `run_started_at` (the latter handles re-runs
/// that overwrite a prior file). Returned in lexicographic order so
/// imports are deterministic across runs.
fn scan_produced_files(
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
        if !(path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md")) {
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
    fn list_md_files_returns_only_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "x").unwrap();
        std::fs::write(dir.path().join("b.txt"), "y").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        let set = list_md_files(dir.path());
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn scan_produced_files_diffs_against_pre_existing() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.md");
        std::fs::write(&stale, "x").unwrap();
        let pre = list_md_files(dir.path());
        let started = SystemTime::now();
        // Sleep so the new file's mtime is provably after `started`.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let fresh = dir.path().join("fresh.md");
        std::fs::write(&fresh, "y").unwrap();
        let produced = scan_produced_files(dir.path(), &pre, started);
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].file_name().unwrap(), "fresh.md");
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
        );
        assert!(prompt.contains("REQ_BODY"));
        assert!(prompt.contains("SKILL_BODY"));
        assert!(prompt.contains("/tmp/x"));
        assert!(prompt.contains("artifact_kind: epic"));
        // No aggregate section when none provided.
        assert!(!prompt.contains("aggregated"));
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
        );
        assert!(prompt.contains("SEED_BODY"));
        assert!(prompt.contains("aggregated task artifacts (2 total)"));
        assert!(prompt.contains("--- artifact: task-01-add-user-table ---"));
        assert!(prompt.contains("Body of task 1"));
        assert!(prompt.contains("--- artifact: task-02-add-login-form ---"));
        assert!(prompt.contains("Body of task 2"));
    }
}
