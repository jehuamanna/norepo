//! Autonomous SDLC cascade orchestrator.
//!
//! Driven by the Play button in the artifact view. Walks the artifact
//! tree breadth-first from a root (typically a Requirements seed),
//! finds every project skill whose declared `input_kind` matches each
//! artifact's kind, runs each matching skill in turn (sequentially —
//! one skill at a time, one source at a time), auto-approves every
//! produced child so the next level's runtime gate doesn't block, and
//! recurses.
//!
//! The orchestrator is a pure async fn that delegates per-skill
//! execution to `runner::run_skill_on_source`; only the *cascade*
//! semantics live here. The view is responsible for spawning,
//! cancellation, and updating the global `CASCADE_STATE`.

#![cfg(not(target_arch = "wasm32"))]

use operon_plugins_claude_code::ClaudeCodeChatPlugin;
use operon_store::repos::{
    ChatMessageRepository, LocalNoteRepository, LocalProjectRepository, NoteKind,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::plugins::artifact::cascade_graph::{
    parse_cross_tree_deps, parse_depends_on, CascadeGraphWriter,
};
use crate::plugins::artifact::frontmatter::{
    parse as parse_artifact_fm, rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
};
use crate::plugins::artifact::runner::{run_skill_on_source, RunnerError};
use crate::plugins::skill::frontmatter::{
    contract as parse_skill_contract, split as split_skill, SkillContract,
};
use operon_store::repos::LocalNote;
use crate::shell::companion_state::{CascadePhase, CASCADE_STATE};

#[derive(Debug, Clone)]
pub enum CascadeOutcome {
    Completed { artifacts_produced: usize },
    Cancelled { artifacts_produced: usize },
}

#[derive(Debug)]
pub enum CascadeError {
    NotFound(String),
    SkillRun(String),
    Io(String),
}

impl std::fmt::Display for CascadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "not found: {s}"),
            Self::SkillRun(s) => write!(f, "skill run failed: {s}"),
            Self::Io(s) => write!(f, "io: {s}"),
        }
    }
}

/// Snapshot of one project skill, captured up front so the cascade
/// doesn't re-load skill bodies on every level. The `id` is the
/// skill's note id (passed to `run_skill_on_source`); the contract is
/// parsed once.
#[derive(Debug, Clone)]
pub struct SkillRef {
    pub id: Uuid,
    pub title: String,
    pub contract: SkillContract,
}

/// Drive the autonomous cascade. Returns when the queue is empty or
/// `cancel` fires; both are reported via the `CascadeOutcome` variant.
///
/// `enabled_skill_ids` is the user's checkbox selection from the
/// stages dropdown — only skills in this set participate. An empty set
/// means *no* skills run; the cascade returns Completed with zero
/// artifacts produced.
#[allow(clippy::too_many_arguments)]
pub async fn run_cascade(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: &Arc<dyn LocalProjectRepository>,
    persistence: &Arc<dyn Persistence>,
    plugin: &Arc<ClaudeCodeChatPlugin>,
    chat_message_repo: &Arc<dyn ChatMessageRepository>,
    project_id: Uuid,
    root_artifact_id: Uuid,
    enabled_skill_ids: HashSet<Uuid>,
    cancel: CancellationToken,
    graph_writer: Option<&mut CascadeGraphWriter>,
) -> Result<CascadeOutcome, CascadeError> {
    // 1. Snapshot every project skill, drop the ones not in the
    //    enabled set, parse contracts. One-shot — skill bodies don't
    //    change mid-cascade.
    let skills = load_project_skills(note_repo, persistence, project_id, &enabled_skill_ids).await;
    let by_input = group_by_input_kind(&skills);

    let mut graph_writer = graph_writer;
    let mut queue: VecDeque<(Uuid, u32)> = VecDeque::from([(root_artifact_id, 0u32)]);
    let mut produced: usize = 0;
    // Title-by-artifact map collected as we go — handed to the
    // graph writer's Depends-on second pass at the end so cross-
    // edges can resolve "Depends on: T001" / "Depends on: feature-01".
    let mut titles: HashMap<Uuid, String> = HashMap::new();

    // Topological-cascade state. `done` is "this artifact has been
    // fully processed (every matching skill has fired) by this
    // cascade run, OR was Approved before the cascade started" —
    // anything in `done` satisfies dep checks for downstream items.
    // Pre-seed with every Approved artifact in the project so re-runs
    // honor prior progress.
    let mut done: HashSet<Uuid> = pre_existing_approved_artifacts(note_repo, persistence, project_id).await;
    // `deferred` holds artifacts pulled off the queue whose deps
    // weren't all in `done`. Value is the set of unmet dep ids; once
    // all become `done`, the artifact is re-enqueued.
    let mut deferred: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();

    'outer: loop {
        if cancel.is_cancelled() {
            return Ok(CascadeOutcome::Cancelled {
                artifacts_produced: produced,
            });
        }

        let (art_id, level) = match queue.pop_front() {
            Some(x) => x,
            None => {
                if deferred.is_empty() {
                    break 'outer;
                }
                // Queue is empty but items remain deferred → unresolvable
                // deps. Surface a Failed outcome with the stuck items so
                // the user can fix the body or backlog and re-run.
                let stuck: Vec<String> = deferred
                    .iter()
                    .map(|(id, needs)| {
                        let title = titles
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| id.to_string());
                        let need_titles: Vec<String> = needs
                            .iter()
                            .map(|n| {
                                titles.get(n).cloned().unwrap_or_else(|| n.to_string())
                            })
                            .collect();
                        format!("{title} <- [{}]", need_titles.join(", "))
                    })
                    .collect();
                return Err(CascadeError::SkillRun(format!(
                    "cascade deadlocked — {} item(s) waiting on unresolvable deps (likely a cycle): {}",
                    deferred.len(),
                    stuck.join("; ")
                )));
            }
        };

        // Dep gate: before processing this artifact, check that every
        // declared dep (artifact body's `## Depends on` ∪ cross-tree
        // edges from sibling backlogs targeting this artifact) is in
        // `done`. If not, defer until they are.
        let deps = compute_artifact_deps(
            note_repo,
            persistence,
            project_id,
            root_artifact_id,
            art_id,
        )
        .await;
        let unmet: HashSet<Uuid> =
            deps.into_iter().filter(|d| !done.contains(d)).collect();
        if !unmet.is_empty() {
            deferred.insert(art_id, unmet);
            continue 'outer;
        }

        let kind_str = match read_kind(persistence, art_id).await {
            Some(k) => k,
            None => {
                // Not an artifact note — still mark "done" so anything
                // depending on it (unlikely but possible) resolves.
                done.insert(art_id);
                continue;
            }
        };

        let matching = by_input.get(&kind_str).cloned().unwrap_or_default();
        for skill in matching {
            if cancel.is_cancelled() {
                break;
            }
            CASCADE_STATE.with_mut(|m| {
                m.insert(
                    root_artifact_id,
                    CascadePhase::Running {
                        artifact_id: art_id,
                        skill_id: skill.id,
                        level,
                    },
                );
            });

            // Route every cascade skill run through the cascade-wide
            // chat session (not the per-source one). Without this the
            // user's `Cascade: <id>` tab stays empty even while skills
            // are happily streaming, because the runner persists to
            // `chat_session_id_for_source(art_id)` while the tab the
            // user opened was bound on `chat_session_id_for_cascade(root)`
            // — different v5 UUIDs derived from different namespaces.
            // Sharing one session means the whole cascade transcript
            // appears in the one tab the user is watching, in skill
            // firing order.
            let chat_session_id =
                crate::plugins::artifact::view::chat_session_id_for_cascade(root_artifact_id);
            let outcome = run_skill_on_source(
                note_repo,
                project_repo,
                persistence,
                plugin,
                Some(chat_message_repo),
                chat_session_id,
                art_id,
                skill.id,
            )
            .await;

            match outcome {
                Ok(o) => {
                    // Resolve titles + bodies once per child for the
                    // graph writer (also feeds the title map used by
                    // the Depends-on second pass at the end).
                    let project_notes_for_titles =
                        note_repo.list_for_project(project_id).unwrap_or_default();
                    // Checkpoint skills (`cascade_stop: true`) emit
                    // artifacts that the cascade must NOT auto-approve
                    // — they're human-review gates. Children land in
                    // Pending, the cascade does not enqueue them, and
                    // the run ends with a Paused phase so the UI can
                    // surface "review the new backlog and approve to
                    // continue".
                    let checkpoint_hit = skill.contract.cascade_stop
                        && !o.created_artifact_ids.is_empty();
                    for child_id in &o.created_artifact_ids {
                        if !skill.contract.cascade_stop {
                            if let Err(e) = approve_artifact(persistence, *child_id).await {
                                tracing::warn!(
                                    target: "operon::cascade",
                                    "approve_artifact failed for {child_id}: {e}"
                                );
                            }
                        }
                        produced += 1;

                        let child_title = project_notes_for_titles
                            .iter()
                            .find(|n| n.id == *child_id)
                            .map(|n| n.title.clone())
                            .unwrap_or_default();
                        titles.insert(*child_id, child_title.clone());

                        if let Some(writer) = graph_writer.as_deref_mut() {
                            let body = persistence
                                .load(&child_id.to_string())
                                .await
                                .ok()
                                .and_then(|b| String::from_utf8(b).ok())
                                .unwrap_or_default();
                            writer.on_artifact_produced(art_id, *child_id, &child_title, body);
                        }
                    }
                    // Flush the graph after each skill run so the
                    // workflow canvas re-renders live as the cascade
                    // progresses (the user can keep the Cascade
                    // workflow tab open and watch nodes appear).
                    if let Some(writer) = graph_writer.as_deref() {
                        if let Err(e) = writer.flush(persistence).await {
                            tracing::warn!(
                                target: "operon::cascade",
                                "graph flush failed: {e}"
                            );
                        }
                    }
                    if checkpoint_hit {
                        // Surface a Paused phase so the view can
                        // print the "review and approve" status line.
                        // We deliberately do NOT enqueue produced
                        // children — the cascade stops here until
                        // the user approves and re-runs.
                        CASCADE_STATE.with_mut(|m| {
                            m.insert(
                                root_artifact_id,
                                CascadePhase::Paused {
                                    artifact_id: o
                                        .created_artifact_ids
                                        .first()
                                        .copied()
                                        .unwrap_or(art_id),
                                    skill_id: skill.id,
                                    level,
                                },
                            );
                        });
                        // Final flush of any in-flight graph state
                        // before bailing — keeps the canvas honest.
                        if let Some(writer) = graph_writer.as_deref_mut() {
                            writer.finalize_depends_on(&titles);
                            if let Err(e) = writer.flush(persistence).await {
                                tracing::warn!(
                                    target: "operon::cascade",
                                    "graph paused-flush failed: {e}"
                                );
                            }
                        }
                        return Ok(CascadeOutcome::Completed {
                            artifacts_produced: produced,
                        });
                    }
                    for child_id in o.created_artifact_ids {
                        queue.push_back((child_id, level + 1));
                    }
                }
                Err(e) => {
                    return Err(CascadeError::SkillRun(format!(
                        "{} on {}: {e}",
                        skill.title, art_id
                    )));
                }
            }
        }

        // All matching skills have fired on `art_id` without bailing
        // out via cascade_stop. Mark it done so anything deferred on
        // it can unblock.
        done.insert(art_id);

        // Sweep deferred — any item whose unmet set is now fully in
        // `done` re-enters the queue. Removed in a separate pass to
        // avoid mutating while iterating.
        let unblocked: Vec<Uuid> = deferred
            .iter()
            .filter(|(_, needs)| needs.iter().all(|d| done.contains(d)))
            .map(|(id, _)| *id)
            .collect();
        for id in unblocked {
            deferred.remove(&id);
            queue.push_back((id, level + 1));
        }
    }

    // Second pass for the visualization: now that every artifact is
    // on disk with its body, parse `## Depends on` sections and add
    // amber cross-edges between siblings. Then a final flush so the
    // canvas reflects the dependency edges.
    if let Some(writer) = graph_writer.as_deref_mut() {
        writer.finalize_depends_on(&titles);
        if let Err(e) = writer.flush(persistence).await {
            tracing::warn!(
                target: "operon::cascade",
                "graph final flush failed: {e}"
            );
        }
    }

    Ok(CascadeOutcome::Completed {
        artifacts_produced: produced,
    })
}

/// Snapshot every `NoteKind::Skill` note in the project, filter down
/// to the user-enabled set, parse each one's `SkillContract`. Returns
/// in title-alphabetical order so within a level the cascade runs
/// skills deterministically.
pub async fn load_project_skills(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    enabled: &HashSet<Uuid>,
) -> Vec<SkillRef> {
    let mut rows: Vec<_> = match note_repo.list_for_project(project_id) {
        Ok(v) => v.into_iter().filter(|n| matches!(n.kind, NoteKind::Skill)).collect(),
        Err(e) => {
            tracing::warn!(
                target: "operon::cascade",
                "list_for_project({project_id}) failed: {e}"
            );
            return Vec::new();
        }
    };
    rows.sort_by(|a, b| a.title.cmp(&b.title));

    let mut out: Vec<SkillRef> = Vec::with_capacity(rows.len());
    for row in rows {
        if !enabled.contains(&row.id) {
            continue;
        }
        let bytes = match persistence.load(&row.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (lines_opt, _) = split_skill(&body);
        let lines = lines_opt.unwrap_or_default();
        let contract = parse_skill_contract(&lines);
        out.push(SkillRef {
            id: row.id,
            title: row.title,
            contract,
        });
    }
    out
}

/// Index skills by `input_kind` so `run_cascade` can look up matching
/// skills per artifact in O(1). Skills without a declared `input_kind`
/// are dropped from the index — they won't auto-fire in a cascade
/// (manual skill picker still offers them).
pub fn group_by_input_kind(skills: &[SkillRef]) -> HashMap<String, Vec<SkillRef>> {
    let mut out: HashMap<String, Vec<SkillRef>> = HashMap::new();
    for s in skills {
        if let Some(input) = s.contract.input_kind.as_ref() {
            out.entry(input.clone()).or_default().push(s.clone());
        }
    }
    out
}

/// Snapshot every Artifact note in the project whose status was
/// already `Approved` before the cascade started. Pre-seeds the
/// topological cascade's `done` set so re-runs after partial
/// completion don't re-block on already-finished work.
pub async fn pre_existing_approved_artifacts(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> HashSet<Uuid> {
    let mut out = HashSet::new();
    let notes = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return out,
    };
    for note in notes {
        if !matches!(note.kind, NoteKind::Artifact) {
            continue;
        }
        let bytes = match persistence.load(&note.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fm = parse_artifact_fm(&body);
        if fm.status == ArtifactStatus::Approved {
            out.insert(note.id);
        }
    }
    out
}

/// Compute the set of artifact ids `art_id` depends on. Sources:
/// - `art_id`'s own body `## Depends on` slugs.
/// - Cross-tree edges in any `prioritized_backlog` artifact under the
///   `seed_id` subtree where the dependent slug resolves to `art_id`.
///
/// Slugs are resolved against the project-wide artifact-title index
/// (full title or first whitespace-delimited token of the title).
/// Unresolved slugs are silently dropped — they get logged at warn
/// level for the user to notice but don't deadlock the cascade.
pub async fn compute_artifact_deps(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
    art_id: Uuid,
) -> HashSet<Uuid> {
    let notes = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return HashSet::new(),
    };
    // Title → id index, including TaskID-style first-token alias.
    let mut by_title: HashMap<String, Uuid> = HashMap::new();
    for n in &notes {
        if !matches!(n.kind, NoteKind::Artifact) {
            continue;
        }
        by_title.insert(n.title.clone(), n.id);
        if let Some(first) = n.title.split_whitespace().next() {
            by_title.entry(first.to_string()).or_insert(n.id);
        }
    }

    // Title for art_id (used to filter cross-tree edges that target
    // art_id). Same alias rule as the index.
    let art_title = notes
        .iter()
        .find(|n| n.id == art_id)
        .map(|n| n.title.clone())
        .unwrap_or_default();
    let art_first_token: String = art_title
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    let art_matches = |slug: &str| -> bool {
        slug == art_title || (!art_first_token.is_empty() && slug == art_first_token)
    };

    // Build the seed's descendant id set so we only scan backlogs
    // under the cascade's root. (Backlogs in unrelated trees in the
    // same project don't influence this cascade.)
    let descendants = subtree_ids(&notes, seed_id);

    let mut deps: HashSet<Uuid> = HashSet::new();

    // (1) art_id's own `## Depends on` body slugs.
    if let Ok(bytes) = persistence.load(&art_id.to_string()).await {
        if let Ok(body) = String::from_utf8(bytes) {
            for slug in parse_depends_on(&body) {
                if let Some(dep_id) = by_title.get(&slug) {
                    if *dep_id != art_id {
                        deps.insert(*dep_id);
                    }
                } else {
                    tracing::warn!(
                        target: "operon::cascade",
                        "unresolved `## Depends on` slug '{slug}' on artifact {art_id}"
                    );
                }
            }
        }
    }

    // (2) Cross-tree edges from prioritized_backlog artifacts under
    //     the seed's subtree where the dependent slug == art_id.
    for note in &notes {
        if !matches!(note.kind, NoteKind::Artifact) || !descendants.contains(&note.id) {
            continue;
        }
        let bytes = match persistence.load(&note.id.to_string()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fm = parse_artifact_fm(&body);
        let is_backlog = fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == "prioritized_backlog")
            .unwrap_or(false);
        if !is_backlog {
            continue;
        }
        for (dependent, prerequisite) in parse_cross_tree_deps(&body) {
            if !art_matches(&dependent) {
                continue;
            }
            if let Some(dep_id) = by_title.get(&prerequisite) {
                if *dep_id != art_id {
                    deps.insert(*dep_id);
                }
            } else {
                tracing::warn!(
                    target: "operon::cascade",
                    "unresolved cross-tree dep '{dependent} -> {prerequisite}' \
                     in backlog {} (under seed {seed_id})",
                    note.id
                );
            }
        }
    }

    deps
}

/// All note ids reachable from `seed_id` via the `parent_id` chain
/// (the seed itself plus every descendant). Used to scope dep
/// scanning to the current cascade's tree.
fn subtree_ids(notes: &[LocalNote], seed_id: Uuid) -> HashSet<Uuid> {
    let mut by_parent: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for n in notes {
        if let Some(p) = n.parent_id {
            by_parent.entry(p).or_default().push(n.id);
        }
    }
    let mut out = HashSet::new();
    let mut queue: VecDeque<Uuid> = VecDeque::new();
    queue.push_back(seed_id);
    out.insert(seed_id);
    while let Some(id) = queue.pop_front() {
        if let Some(children) = by_parent.get(&id) {
            for c in children {
                if out.insert(*c) {
                    queue.push_back(*c);
                }
            }
        }
    }
    out
}

/// Read just the artifact_kind off an artifact note's frontmatter.
/// Returns the `as_str()` form (e.g. "epic") for index keying.
/// Returns `None` if the note isn't an artifact / has no kind / can't
/// be loaded.
pub async fn read_kind(persistence: &Arc<dyn Persistence>, id: Uuid) -> Option<String> {
    let bytes = persistence.load(&id.to_string()).await.ok()?;
    let body = String::from_utf8(bytes).ok()?;
    let fm = parse_artifact_fm(&body);
    fm.artifact_kind.map(|k| k.as_str().to_string())
}

/// Flip an artifact's status to Approved on disk so downstream skills
/// pass the runtime gate. Loads the body, rewrites frontmatter, saves.
/// Idempotent — already-Approved artifacts are touched but unchanged.
pub async fn approve_artifact(
    persistence: &Arc<dyn Persistence>,
    artifact_id: Uuid,
) -> Result<(), CascadeError> {
    let bytes = persistence
        .load(&artifact_id.to_string())
        .await
        .map_err(|e| CascadeError::NotFound(format!("load {artifact_id}: {e}")))?;
    let body = String::from_utf8(bytes)
        .map_err(|e| CascadeError::Io(format!("utf8 {artifact_id}: {e}")))?;
    let mut fm = parse_artifact_fm(&body);
    if fm.status == ArtifactStatus::Approved {
        return Ok(());
    }
    fm.status = ArtifactStatus::Approved;
    let new_body = rewrite_artifact_fm(&body, &fm);
    persistence
        .save(&artifact_id.to_string(), new_body.as_bytes())
        .await
        .map_err(|e| CascadeError::Io(format!("save {artifact_id}: {e}")))?;
    Ok(())
}

// Silences `unused_import` warnings in builds that don't exercise
// every helper (e.g. wasm-cfg permutations). All re-exports are part
// of the orchestrator's public surface.
#[allow(dead_code)]
fn _force_pub_use(_e: ArtifactKind, _r: RunnerError) {}

/// JSON sidecar stored at `<repo>/.operon/cascade-stages.json` that
/// records which skill ids are enabled for cascade runs in this
/// project. Absent file = "all skills enabled" (the StagesDropdown
/// renders every checkbox on by default). Present file with empty
/// array = "no skills enabled" (Play does nothing — the user has
/// explicitly opted out of every stage).
///
/// Stored on the project's repo path rather than in SQLite so we
/// don't need a migration; per-project follows the project's
/// repository naturally.
pub mod stages_sidecar {
    use super::*;
    use std::path::{Path, PathBuf};

    fn sidecar_path(repo_path: &Path) -> PathBuf {
        repo_path.join(".operon").join("cascade-stages.json")
    }

    /// Read the enabled-skill set. Returns `None` when the file is
    /// missing — caller should treat as "all skills enabled".
    pub fn load(repo_path: &Path) -> Option<HashSet<Uuid>> {
        let path = sidecar_path(repo_path);
        let bytes = std::fs::read(&path).ok()?;
        let ids: Vec<Uuid> = serde_json::from_slice(&bytes).ok()?;
        Some(ids.into_iter().collect())
    }

    /// Write the enabled-skill set. Creates `.operon/` if missing.
    pub fn save(repo_path: &Path, enabled: &HashSet<Uuid>) -> std::io::Result<()> {
        let path = sidecar_path(repo_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut ids: Vec<Uuid> = enabled.iter().copied().collect();
        ids.sort(); // deterministic on-disk order so diffs are stable
        let json = serde_json::to_vec_pretty(&ids)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, json)
    }

    /// Convenience: resolve enabled set for a cascade run. Falls back
    /// to "every project skill enabled" when the sidecar is absent.
    /// `all_skill_ids` is the full set of project skill ids (we
    /// expand "no sidecar" to "everything" so a fresh project just
    /// works).
    pub fn resolve_or_all(
        repo_path: &Path,
        all_skill_ids: &HashSet<Uuid>,
    ) -> HashSet<Uuid> {
        load(repo_path).unwrap_or_else(|| all_skill_ids.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::skill::frontmatter::{SkillGate, SkillOutputCount};

    fn skill_ref(id_seed: u8, title: &str, input: &str, output: &str) -> SkillRef {
        SkillRef {
            id: Uuid::from_bytes([id_seed; 16]),
            title: title.into(),
            contract: SkillContract {
                input_kind: Some(input.into()),
                output_kind: Some(output.into()),
                output_count: SkillOutputCount::Many,
                gate: SkillGate::Approval,
                persona: None,
                ..SkillContract::default()
            },
        }
    }

    #[test]
    fn group_by_input_kind_indexes_each_skill() {
        let skills = vec![
            skill_ref(1, "ba-decompose-features", "epic", "feature"),
            skill_ref(2, "ba-discover-epics", "requirements", "epic"),
            skill_ref(3, "sa-design-feature-hld", "feature", "plan"),
        ];
        let idx = group_by_input_kind(&skills);
        assert_eq!(idx.get("epic").map(|v| v.len()), Some(1));
        assert_eq!(idx.get("requirements").map(|v| v.len()), Some(1));
        assert_eq!(idx.get("feature").map(|v| v.len()), Some(1));
        assert!(idx.get("story").is_none());
    }

    #[test]
    fn group_by_input_kind_collects_multiple_per_input() {
        // Both BA stories and SA feature-HLD consume `feature` →
        // they must both end up in the index for cascade to fan out.
        let skills = vec![
            skill_ref(1, "ba-decompose-stories", "feature", "story"),
            skill_ref(2, "sa-design-feature-hld", "feature", "plan"),
        ];
        let idx = group_by_input_kind(&skills);
        let bucket = idx.get("feature").expect("feature input has skills");
        assert_eq!(bucket.len(), 2);
    }

    #[test]
    fn skipped_skills_drop_from_index_when_no_input_kind() {
        let mut weird = skill_ref(1, "no-input", "ignored", "ignored");
        weird.contract.input_kind = None;
        let idx = group_by_input_kind(&[weird]);
        assert!(idx.is_empty());
    }

    fn note(id: Uuid, parent: Option<Uuid>, title: &str) -> LocalNote {
        LocalNote {
            id,
            project_id: Uuid::nil(),
            parent_id: parent,
            sibling_index: 0,
            depth: 0,
            title: title.into(),
            created_at_ms: 0,
            updated_at_ms: 0,
            kind: NoteKind::Artifact,
            blob_path: None,
        }
    }

    #[test]
    fn subtree_ids_includes_seed_and_all_descendants() {
        let seed = Uuid::from_bytes([1; 16]);
        let epic_a = Uuid::from_bytes([2; 16]);
        let epic_b = Uuid::from_bytes([3; 16]);
        let feat_a1 = Uuid::from_bytes([4; 16]);
        let feat_a2 = Uuid::from_bytes([5; 16]);
        let unrelated = Uuid::from_bytes([6; 16]);
        let notes = vec![
            note(seed, None, "Requirements"),
            note(epic_a, Some(seed), "Epic A"),
            note(epic_b, Some(seed), "Epic B"),
            note(feat_a1, Some(epic_a), "Feature A.1"),
            note(feat_a2, Some(epic_a), "Feature A.2"),
            note(unrelated, None, "Unrelated note"),
        ];
        let ids = subtree_ids(&notes, seed);
        assert_eq!(ids.len(), 5);
        assert!(ids.contains(&seed));
        assert!(ids.contains(&epic_a));
        assert!(ids.contains(&epic_b));
        assert!(ids.contains(&feat_a1));
        assert!(ids.contains(&feat_a2));
        assert!(!ids.contains(&unrelated));
    }

    #[test]
    fn subtree_ids_handles_seed_with_no_children() {
        let seed = Uuid::from_bytes([1; 16]);
        let notes = vec![note(seed, None, "Requirements")];
        let ids = subtree_ids(&notes, seed);
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&seed));
    }

    #[test]
    fn subtree_ids_returns_only_seed_when_seed_unknown() {
        // No matching note for `seed` — function still returns the
        // seed id itself (cascade callers always start with a real
        // root, but we don't want the helper to surprise them by
        // returning empty when the row hasn't loaded yet).
        let seed = Uuid::from_bytes([42; 16]);
        let other = Uuid::from_bytes([99; 16]);
        let notes = vec![note(other, None, "Some other note")];
        let ids = subtree_ids(&notes, seed);
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&seed));
    }
}
