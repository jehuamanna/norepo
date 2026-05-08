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
use crate::plugins::artifact::cascade_graph::CascadeGraphWriter;
use crate::plugins::artifact::frontmatter::{
    parse as parse_artifact_fm, rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
};
use crate::plugins::artifact::runner::{run_skill_on_source, RunnerError};
use crate::plugins::skill::frontmatter::{
    contract as parse_skill_contract, split as split_skill, SkillContract,
};
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

    while let Some((art_id, level)) = queue.pop_front() {
        if cancel.is_cancelled() {
            return Ok(CascadeOutcome::Cancelled {
                artifacts_produced: produced,
            });
        }

        let kind_str = match read_kind(persistence, art_id).await {
            Some(k) => k,
            None => continue, // not an artifact note (no frontmatter / wrong kind)
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

            let chat_session_id = crate::plugins::artifact::view::chat_session_id_for_source(art_id);
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
                    for child_id in &o.created_artifact_ids {
                        if let Err(e) = approve_artifact(persistence, *child_id).await {
                            tracing::warn!(
                                target: "operon::cascade",
                                "approve_artifact failed for {child_id}: {e}"
                            );
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
}
