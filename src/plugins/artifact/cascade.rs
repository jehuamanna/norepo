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
use crate::plugins::workflow::state::NodeStatus;
use crate::plugins::artifact::frontmatter::{
    parse as parse_artifact_fm, rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
};
use crate::plugins::artifact::runner::{
    run_skill_on_source, run_skill_on_source_with_revision_notes, RunnerError,
};
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

/// Which arm of the SDLC pipeline the current cascade is running.
/// Determined once per `run_cascade` from the root artifact's kind:
///
///   - `Ba`: cascade rooted on a `master_requirement` (or legacy
///     `requirements` root). Runs the BA chain plus
///     `06-sa-draft-architecture`. SDE-chain skills are filtered out
///     so the cascade naturally stops once Architecture is produced.
///   - `Sde`: cascade rooted on a `task`. Runs only the SDE chain
///     (`implementation` → `test_cases` → `test_results`, plus
///     `bug` → fix-bug regen). BA-chain and Architecture skills are
///     filtered out.
///   - `Mixed`: any other root — run every enabled skill (legacy
///     behavior, for back-compat with the seed-skills-employee chain
///     and ad-hoc roots).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillPhase {
    Ba,
    Sde,
    Mixed,
}

impl SkillPhase {
    /// Pick the phase based on the cascade root's `artifact_kind`
    /// string. `None` (or any unrecognised kind) falls back to
    /// `Mixed` so old-style cascades keep working.
    pub fn for_root_kind(kind: Option<&str>) -> Self {
        match kind {
            Some("master_requirement") => Self::Ba,
            Some("requirements") => Self::Ba,
            Some("task") => Self::Sde,
            _ => Self::Mixed,
        }
    }
}

/// `true` when a skill's `input_kind` belongs to the BA arm of the
/// pipeline (master_requirement, requirements, epic, feature, story).
/// Architecture skills (`input_kind: master_requirement` →
/// `output_kind: architecture`) fall in this set too because they
/// run as part of the master-driven phase.
fn is_ba_input_kind(kind: &str) -> bool {
    matches!(
        kind,
        "master_requirement"
            | "requirements"
            | "epic"
            | "feature"
            | "story"
    )
}

/// `true` when a skill's `input_kind` belongs to the SDE arm
/// (task, implementation, test_cases, bug, test_results). Test
/// results' downstream skills (e.g. legacy `10-sum-summarize-task`)
/// stay in the SDE arm.
fn is_sde_input_kind(kind: &str) -> bool {
    matches!(
        kind,
        "task" | "implementation" | "test_cases" | "test_results" | "bug"
    )
}

/// `true` when the cascade should pre-flight check for at least one
/// `requirements` artifact under the cascade root. Triggers only on a
/// BA-phase run rooted on a `master_requirement` — legacy
/// `requirements`-root cascades and SDE-phase task runs don't apply.
/// Extracted from the inline gate logic so it stays unit-testable.
pub fn needs_requirements_gate(phase: SkillPhase, root_kind: Option<&str>) -> bool {
    matches!(phase, SkillPhase::Ba) && root_kind == Some("master_requirement")
}

/// Human-readable error surfaced when the master-requirement
/// readiness gate finds zero `requirements` descendants. Phrased as
/// a fix-it instruction so the BA can resolve it without leaving
/// the cascade-status row.
pub fn empty_requirements_message() -> String {
    "No `requirements` artifacts exist under this master_requirement. \
     Right-click the master in the explorer \u{2192} Add child note, \
     then set `artifact_kind: requirements` + `status: approved` in \
     the new note's frontmatter. Add as many as you need, then click \
     Play again."
        .into()
}

/// Drop skills whose `input_kind` doesn't belong to the active
/// phase. Skills with no declared `input_kind` always survive — they
/// run at any phase as utility skills (e.g. a global summarizer).
pub fn filter_skills_for_phase(skills: Vec<SkillRef>, phase: SkillPhase) -> Vec<SkillRef> {
    if matches!(phase, SkillPhase::Mixed) {
        return skills;
    }
    skills
        .into_iter()
        .filter(|s| {
            let Some(input_kind) = s.contract.input_kind.as_deref() else {
                return true;
            };
            match phase {
                SkillPhase::Ba => is_ba_input_kind(input_kind),
                SkillPhase::Sde => is_sde_input_kind(input_kind),
                SkillPhase::Mixed => true,
            }
        })
        .collect()
}

/// Drive the autonomous cascade. Returns when the queue is empty or
/// `cancel` fires; both are reported via the `CascadeOutcome` variant.
///
/// `enabled_skill_ids` is the user's checkbox selection from the
/// stages dropdown — only skills in this set participate. An empty set
/// means *no* skills run; the cascade returns Completed with zero
/// artifacts produced.
///
/// `max_depth` bounds how far down the BFS the cascade walks before
/// stopping. `None` is unbounded (the original full-cascade behavior);
/// `Some(1)` runs skills only on the root and enqueues — but does not
/// process — its direct children, which is the "one step at a time"
/// progression the workflow card's ▶ button uses to walk the SDLC
/// pipeline level by level.
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
    max_depth: Option<u32>,
    cascade_session_id: Uuid,
) -> Result<CascadeOutcome, CascadeError> {
    // 0. Clarification gate. The `00-coherence-check` skill produces
    //    `clarification` artifacts when it detects cross-level
    //    discrepancies. Any clarification left in `Pending` is a hard
    //    signal that the user owes the pipeline a decision before
    //    downstream work re-runs — refuse to start so the user can't
    //    accidentally produce work that contradicts what they're
    //    about to answer. The fix-up motion is: open each unresolved
    //    clarification, submit an answer (which appends an
    //    `## Answer` block, flips status to Approved, and writes the
    //    resolved direction into each `## Resolution target`'s
    //    `revision_notes`). Then click Play again.
    let pending_clarifications =
        unresolved_clarification_titles(note_repo, persistence, project_id).await;
    if !pending_clarifications.is_empty() {
        return Err(CascadeError::SkillRun(format!(
            "{} unresolved clarification(s) blocking cascade — answer them first: {}",
            pending_clarifications.len(),
            pending_clarifications.join(", ")
        )));
    }

    // 1. Snapshot every project skill, drop the ones not in the
    //    enabled set, parse contracts. One-shot — skill bodies don't
    //    change mid-cascade.
    let skills = load_project_skills(note_repo, persistence, project_id, &enabled_skill_ids).await;

    // 1a. Phase filter. The SDLC pipeline splits at the Architecture
    //     boundary: master_requirement runs (and any descendant up
    //     through `task`) trigger the BA chain + Architecture only,
    //     stopping before SDE skills fire. Task runs trigger the SDE
    //     chain only. This keeps the per-task Play button surgical
    //     (just THIS task's chain) and the master Play meaningful
    //     (produce the spec, hand off to engineering).
    //
    //     The filter is driven by the cascade root's kind, read once
    //     here. Skills whose `input_kind` falls outside the active
    //     phase are dropped before `group_by_input_kind`.
    let root_kind_str = read_kind(persistence, root_artifact_id).await;
    let phase = SkillPhase::for_root_kind(root_kind_str.as_deref());
    let skills = filter_skills_for_phase(skills, phase);
    let by_input = group_by_input_kind(&skills);

    // 1b. Master-requirement readiness gate. The BA hand-authors each
    //     detail Requirement under master_requirement; there's no AI
    //     step that produces them. If the user clicks Play with zero
    //     `requirements` children, the empty-aggregation gate further
    //     down (`count_descendant_artifacts_of_kind` check on each
    //     aggregator skill) would silently skip
    //     `02-ba-discover-epics` + `06-sa-draft-architecture` and the
    //     cascade would complete with no work done. That's a
    //     confusing failure mode; halt loudly here instead so the BA
    //     gets a clear message in the cascade-status row.
    if needs_requirements_gate(phase, root_kind_str.as_deref()) {
        let req_count = count_descendant_artifacts_of_kind(
            note_repo,
            persistence,
            project_id,
            root_artifact_id,
            "requirements",
        )
        .await;
        if req_count == 0 {
            return Err(CascadeError::SkillRun(empty_requirements_message()));
        }
    }

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

        // Depth cap. `max_depth = Some(1)` makes the workflow card's
        // ▶ button advance one level per click — the root runs its
        // skills (producing children), the children get popped at
        // level 1 and short-circuit here without firing skills.
        if let Some(max) = max_depth {
            if level >= max {
                done.insert(art_id);
                continue 'outer;
            }
        }

        // Regenerate-on-Dirty pre-clean: if the explicit play target
        // (the artifact the user just clicked Play on) is `Dirty`,
        // wipe its existing descendants and flip it back to Pending
        // before the dedup branch below sees the old children. The
        // dedup at "skip-already-produced (artifact, skill) pairs"
        // would otherwise short-circuit and reuse the stale outputs;
        // by deleting first, we force fresh skill runs. Only the
        // explicit play target triggers this — Dirty descendants
        // (children of an Approved root) take the approval gate
        // below and get skipped, which is the resume semantics we
        // already documented.
        if art_id == root_artifact_id {
            let is_dirty = matches!(
                read_status(persistence, art_id).await,
                Some(ArtifactStatus::Dirty)
            );
            if is_dirty {
                let all_notes = note_repo
                    .list_for_project(project_id)
                    .unwrap_or_default();
                let descendants_set = subtree_ids(&all_notes, art_id);
                let mut deleted = 0usize;
                for child_id in descendants_set {
                    if child_id == art_id {
                        continue; // keep the parent; only sweep its tree
                    }
                    if let Err(e) = persistence.delete(&child_id.to_string()).await {
                        tracing::warn!(
                            target: "operon::cascade",
                            "regenerate-on-dirty: persistence.delete({child_id}) failed: {e}"
                        );
                    }
                    if let Err(e) = note_repo.delete(child_id) {
                        tracing::warn!(
                            target: "operon::cascade",
                            "regenerate-on-dirty: note_repo.delete({child_id}) failed: {e}"
                        );
                    } else {
                        deleted += 1;
                    }
                }
                tracing::info!(
                    target: "operon::cascade",
                    "regenerate-on-dirty: wiped {deleted} descendant(s) under {art_id}"
                );
                // Flip the parent's status from Dirty → Pending so a
                // second click on the same artifact (after this run
                // produces fresh children) doesn't trigger another
                // wipe. Pending is the natural pre-Approval state.
                if let Ok(bytes) = persistence.load(&art_id.to_string()).await {
                    if let Ok(body) = String::from_utf8(bytes) {
                        let mut fm = parse_artifact_fm(&body);
                        if matches!(fm.status, ArtifactStatus::Dirty) {
                            fm.status = ArtifactStatus::Pending;
                            let new_body = rewrite_artifact_fm(&body, &fm);
                            let _ = persistence
                                .save(&art_id.to_string(), new_body.as_bytes())
                                .await;
                        }
                    }
                }
            }
        }

        // Approval gate: non-root artifacts must be `Approved` for the
        // cascade to run skills on them. Lets the user approve a subset
        // of newly-produced children and re-click Play (at any level)
        // to walk only the approved subtrees. The explicit play
        // target (`root_artifact_id`) is exempt — it's the entry point
        // the user just clicked, so it always runs (the toolbar's
        // `is_runnable_source` check already gated the click on
        // Approved-or-root anyway). Skipped artifacts stay untouched
        // visually — the existing status pill on the snapshot already
        // surfaces "Pending" / "Rejected" without extra UI.
        if art_id != root_artifact_id {
            let approved = matches!(
                read_status(persistence, art_id).await,
                Some(ArtifactStatus::Approved)
            );
            if !approved {
                tracing::debug!(
                    target: "operon::cascade",
                    "approval gate: skipping {art_id} (not Approved)"
                );
                continue 'outer;
            }
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

            // Skip-already-produced gate. On a cascade re-run after
            // the user approved a checkpoint, the seed pops with all
            // its matching skills (e.g. 01 + 01b) still "matching" —
            // re-firing them would regenerate Epics, regenerate the
            // backlog, hit cascade_stop on 01b, and pause forever
            // without ever reaching downstream tiers. Detect prior
            // output by walking the source's existing children for
            // ones whose frontmatter `source_skill_id` matches this
            // skill. If any exist, treat the (artifact, skill) pair
            // as already done — enqueue the existing children for
            // further walking and skip the run.
            //
            // Exception: if any of those existing children is `Dirty`
            // (the user added Refinement notes and clicked Mark dirty
            // on a descendant, then clicked Play on an ancestor),
            // wipe the dirty child's stale subtree and FALL THROUGH
            // to re-run the skill — passing the dirty child's
            // `revision_notes` as `extra_revision_notes` so they
            // reach the regen prompt. The runner's title-based dedup
            // overwrites the dirty child(ren) in place; the runner
            // also clears the inlined notes after a successful run.
            let already_produced = existing_children_with_skill(
                note_repo,
                persistence,
                project_id,
                art_id,
                skill.id,
            )
            .await;
            // Load each existing child's body once, then run the
            // pure-fn dirty detector on the bundle. Splitting load
            // (impure) from detection (pure) keeps the dirty/regen
            // selection unit-testable without spinning up a real
            // persistence layer.
            let mut child_bodies: Vec<(Uuid, String)> =
                Vec::with_capacity(already_produced.len());
            for child_id in &already_produced {
                if let Ok(bytes) = persistence.load(&child_id.to_string()).await {
                    if let Ok(body) = String::from_utf8(bytes) {
                        child_bodies.push((*child_id, body));
                    }
                }
            }
            let (any_dirty, dirty_notes) = select_dirty_regen_seed(&child_bodies);
            if !already_produced.is_empty() && !any_dirty {
                for child_id in &already_produced {
                    queue.push_back((*child_id, level + 1));
                }
                continue;
            }
            // Captured prior (title, body) pairs of children that
            // are about to be overwritten by this skill rerun, fed
            // to the runner as `previous_outputs` so the regen prompt
            // can honor the seed-skill revision-history convention
            // (append `## Revision N` rows; stash prior body under a
            // collapsed `<details>` block) rather than discarding it.
            // Populated only on the dirty-regen branch — fresh runs
            // have nothing to preserve.
            let mut previous_outputs: Vec<(String, String)> = Vec::new();
            if any_dirty {
                // Wipe each dirty child's subtree — its descendants
                // were derived from the about-to-be-regenerated body
                // and would be stale after the rerun. Keep the dirty
                // node itself; the runner's import dedup overwrites
                // it in place by title. Use the already-loaded
                // `child_bodies` so we don't re-read status from
                // disk.
                let project_notes_snapshot =
                    note_repo.list_for_project(project_id).unwrap_or_default();
                // Index titles by id once so we can resolve each
                // child's title in O(1) below.
                let title_by_id: std::collections::HashMap<Uuid, String> =
                    project_notes_snapshot
                        .iter()
                        .map(|n| (n.id, n.title.clone()))
                        .collect();
                // Capture every child being regenerated (Dirty AND
                // non-Dirty siblings whose body the prompt should
                // preserve). The seed-skill prompts re-emit every
                // sibling on a fan-out run, so any non-dirty body
                // whose subtree we keep would otherwise be silently
                // replaced.
                for (child_id, body) in &child_bodies {
                    if let Some(title) = title_by_id.get(child_id) {
                        previous_outputs.push((title.clone(), body.clone()));
                    }
                }
                let mut wiped: usize = 0;
                for (child_id, body) in &child_bodies {
                    if !matches!(parse_artifact_fm(body).status, ArtifactStatus::Dirty) {
                        continue;
                    }
                    for desc_id in subtree_ids(&project_notes_snapshot, *child_id) {
                        if desc_id == *child_id {
                            continue;
                        }
                        if let Err(e) = persistence.delete(&desc_id.to_string()).await {
                            tracing::warn!(
                                target: "operon::cascade",
                                "dirty-regen: persistence.delete({desc_id}) failed: {e}"
                            );
                        }
                        if let Err(e) = note_repo.delete(desc_id) {
                            tracing::warn!(
                                target: "operon::cascade",
                                "dirty-regen: note_repo.delete({desc_id}) failed: {e}"
                            );
                        } else {
                            wiped += 1;
                        }
                    }
                }
                tracing::info!(
                    target: "operon::cascade",
                    "dirty-regen on {art_id} via skill {}: wiped {wiped} stale descendant(s) before rerun; captured {} prior body(ies) for revision-history preservation",
                    skill.title,
                    previous_outputs.len()
                );
            }

            // Empty-aggregation gate. Aggregator skills (`aggregate:
            // <kind>`) are meaningful only when descendants of the
            // declared kind exist under this artifact. Firing 04b on
            // the seed at the start of a cascade — when no Tasks
            // exist yet — produces a "phantom" Rejected backlog and,
            // worse, hits cascade_stop and pauses the run on
            // nothing-to-prioritize. Skip silently when the
            // aggregation would be empty; the user can run the
            // aggregator manually once descendants exist (or build a
            // post_pass mechanism in a follow-up).
            if let Some(agg_kind) = skill.contract.aggregate.as_deref() {
                let count = count_descendant_artifacts_of_kind(
                    note_repo,
                    persistence,
                    project_id,
                    art_id,
                    agg_kind,
                )
                .await;
                if count == 0 {
                    tracing::debug!(
                        target: "operon::cascade",
                        "skipping aggregator skill {} on {art_id}: no `{agg_kind}` descendants yet",
                        skill.title
                    );
                    continue;
                }
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

            // Mark the source artifact's snapshot in the workflow
            // graph as Running so the canvas surfaces a spinner on
            // the tile that's currently feeding the active skill.
            // Flush so the canvas's WORKFLOW_GRAPH_VERSION watcher
            // re-reads the body and re-renders.
            if let Some(writer) = graph_writer.as_deref_mut() {
                writer.mark_artifact_status(art_id, NodeStatus::Running);
                if let Err(e) = writer.flush(persistence).await {
                    tracing::warn!(
                        target: "operon::cascade",
                        "graph mark-running flush failed: {e}"
                    );
                }
            }

            // Route every cascade skill run through the per-Play-click
            // chat session passed in by the caller (`spawn_cascade`).
            // Each Play click mints a fresh session UUID so two
            // simultaneous cascades don't share a transcript; the
            // cascade orchestrator picks up the same id we registered
            // for the "Claude is working…" indicator and the rail
            // session label.
            //
            // When the dirty-descendant branch above collected
            // refinement notes off a prior output, route through the
            // _with_revision_notes variant so the notes reach the
            // regen prompt under `--- refinement notes from user ---`.
            // The plain `run_skill_on_source` is the unmodified path
            // for the common (no-dirty) case.
            let outcome = if dirty_notes.is_some() || !previous_outputs.is_empty() {
                run_skill_on_source_with_revision_notes(
                    note_repo,
                    project_repo,
                    persistence,
                    plugin,
                    Some(chat_message_repo),
                    cascade_session_id,
                    art_id,
                    skill.id,
                    dirty_notes.clone(),
                    previous_outputs.clone(),
                )
                .await
            } else {
                run_skill_on_source(
                    note_repo,
                    project_repo,
                    persistence,
                    plugin,
                    Some(chat_message_repo),
                    cascade_session_id,
                    art_id,
                    skill.id,
                )
                .await
            };

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
                    //
                    // Step mode (read off the cascade workflow note's
                    // view_state) widens this: every skill that
                    // produced artifacts becomes a checkpoint, so the
                    // user can review each stage independently before
                    // continuing. Defaults off when there's no graph
                    // writer or the user hasn't enabled it.
                    let step_mode_on = graph_writer
                        .as_deref()
                        .map(|w| {
                            crate::plugins::workflow::state::effective_step_mode(&w.graph)
                        })
                        .unwrap_or(false);
                    let checkpoint_hit = (skill.contract.cascade_stop || step_mode_on)
                        && !o.created_artifact_ids.is_empty();
                    for child_id in &o.created_artifact_ids {
                        if !skill.contract.cascade_stop && !step_mode_on {
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
                    // Also clear the source's Running marker — the
                    // skill on this artifact is done.
                    if let Some(writer) = graph_writer.as_deref_mut() {
                        writer.mark_artifact_status(art_id, NodeStatus::Fresh);
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
                            writer.finalize_cross_tree_deps(&titles);
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
                    let msg = format!("{} on {}: {e}", skill.title, art_id);
                    if let Some(writer) = graph_writer.as_deref_mut() {
                        writer.mark_artifact_status(art_id, NodeStatus::Error(msg.clone()));
                        if let Err(fe) = writer.flush(persistence).await {
                            tracing::warn!(
                                target: "operon::cascade",
                                "graph mark-error flush failed: {fe}"
                            );
                        }
                    }
                    return Err(CascadeError::SkillRun(msg));
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
    // amber cross-edges between siblings. Also walk
    // prioritized_backlog bodies for their `## Cross-tree
    // dependencies` section so the consolidated cross-edges declared
    // by the prioritization skills land on the canvas. Then a final
    // flush so the canvas reflects the dependency edges.
    if let Some(writer) = graph_writer.as_deref_mut() {
        writer.finalize_depends_on(&titles);
        writer.finalize_cross_tree_deps(&titles);
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

/// Collect every `clarification` artifact in the project whose
/// status is `Pending` — these block the cascade until the user
/// submits an answer (which flips the artifact to `Approved` and
/// writes the resolution into each target's `revision_notes`).
/// Returns artifact titles for the error message; `Rejected` and
/// `Approved` clarifications are ignored.
pub async fn unresolved_clarification_titles(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
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
        if matches!(fm.artifact_kind, Some(ArtifactKind::Clarification))
            && matches!(fm.status, ArtifactStatus::Pending)
        {
            out.push(note.title.clone());
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

/// Count descendant Artifact notes of `seed_id` whose
/// `artifact_kind` matches `wanted_kind`. Used by the cascade to
/// skip aggregator skills whose `aggregate:` set would be empty —
/// firing them anyway produces phantom Rejected backlogs and traps
/// the cascade behind cascade_stop on nothing.
async fn count_descendant_artifacts_of_kind(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
    wanted_kind: &str,
) -> usize {
    let notes = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let descendants = subtree_ids(&notes, seed_id);
    let mut count = 0usize;
    for note in &notes {
        if note.id == seed_id || !descendants.contains(&note.id) {
            continue;
        }
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
        if fm
            .artifact_kind
            .as_ref()
            .map(|k| k.as_str() == wanted_kind)
            .unwrap_or(false)
        {
            count += 1;
        }
    }
    count
}

/// Find children of `parent_id` whose artifact frontmatter declares
/// `source_skill_id == skill_id`. Used by the cascade to skip
/// re-firing a (artifact, skill) pair on resume runs when the skill
/// has already produced output. Returns ids in title-alphabetical
/// order so the re-queue is deterministic.
async fn existing_children_with_skill(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    parent_id: Uuid,
    skill_id: Uuid,
) -> Vec<Uuid> {
    let mut notes = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    notes.sort_by(|a, b| a.title.cmp(&b.title));
    let mut out = Vec::new();
    for note in notes {
        if note.parent_id != Some(parent_id) || !matches!(note.kind, NoteKind::Artifact) {
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
        if fm.source_skill_id == Some(skill_id) {
            out.push(note.id);
        }
    }
    out
}

/// Pure-fn slice of the cascade's "skip already-produced gate" that
/// asks: do any of these existing children need a regen pass? Given
/// `(child_id, body)` pairs for every artifact already produced by
/// the current `(art_id, skill)` pair, returns:
///
///   - `any_dirty`: `true` if at least one child has `Dirty` status.
///     The orchestrator uses this to decide between the fast
///     enqueue-and-skip path and the regen path (which wipes stale
///     subtrees and re-runs the skill against `art_id`).
///   - `dirty_notes`: the first dirty child carrying a non-empty
///     `revision_notes`. Plumbed into `run_skill_on_source_with_
///     revision_notes`'s `extra_revision_notes` so the user's
///     refinement notes reach the regen prompt.
///
/// Splitting this off the orchestrator keeps the dirty-detection
/// behaviour unit-testable without standing up a real persistence
/// layer or note repository.
fn select_dirty_regen_seed(
    children: &[(Uuid, String)],
) -> (bool, Option<(Uuid, String)>) {
    let mut any_dirty = false;
    let mut dirty_notes: Option<(Uuid, String)> = None;
    for (child_id, body) in children {
        let fm = parse_artifact_fm(body);
        if !matches!(fm.status, ArtifactStatus::Dirty) {
            continue;
        }
        any_dirty = true;
        if dirty_notes.is_some() {
            continue;
        }
        if let Some(notes) = fm.revision_notes {
            dirty_notes = Some((*child_id, notes));
        }
    }
    (any_dirty, dirty_notes)
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

/// Read the artifact's status (Pending/Approved/Rejected/Dirty/...) off
/// its frontmatter. Returns `None` if the note can't be loaded; treat
/// that as "skip" at the callsite. Used by the cascade BFS approval
/// gate so non-approved children block downstream walking.
pub async fn read_status(
    persistence: &Arc<dyn Persistence>,
    id: Uuid,
) -> Option<ArtifactStatus> {
    let bytes = persistence.load(&id.to_string()).await.ok()?;
    let body = String::from_utf8(bytes).ok()?;
    let fm = parse_artifact_fm(&body);
    Some(fm.status)
}

/// For an artifact `art_id` rooted somewhere under `seed_id`, return
/// the human titles of every prerequisite (from its `## Depends on`
/// body section + sibling prioritized-backlog cross-tree edges) that
/// is NOT currently `ArtifactStatus::Approved`. Empty Vec means the
/// cascade can run on this artifact without the runtime dep gate
/// (`cascade.rs:174-187`) deadlocking.
///
/// The artifact-view's Play button uses this to render its disabled
/// state + tooltip, so the user sees "Approve X first" instead of
/// clicking through to a confusing "cascade deadlocked" error. The
/// runtime gate stays intact as a defensive net for races.
pub async fn unmet_dep_titles(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    seed_id: Uuid,
    art_id: Uuid,
) -> Vec<String> {
    let deps = compute_artifact_deps(note_repo, persistence, project_id, seed_id, art_id).await;
    if deps.is_empty() {
        return Vec::new();
    }
    let notes = match note_repo.list_for_project(project_id) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let title_for = |id: Uuid| -> String {
        notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let mut out: Vec<String> = Vec::new();
    for dep_id in deps {
        let approved = matches!(
            read_status(persistence, dep_id).await,
            Some(ArtifactStatus::Approved)
        );
        if !approved {
            out.push(title_for(dep_id));
        }
    }
    // Stable order so the tooltip text doesn't churn between renders.
    out.sort();
    out
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

    /// Build a minimal artifact body with the given status + optional
    /// revision_notes. Mirrors the on-disk YAML-ish frontmatter the
    /// runner writes; round-trips through `parse_artifact_fm`.
    fn artifact_body(status: ArtifactStatus, notes: Option<&str>) -> String {
        let mut fm = crate::plugins::artifact::frontmatter::ArtifactFrontmatter::default();
        fm.artifact_kind = Some(ArtifactKind::Epic);
        fm.status = status;
        fm.revision_notes = notes.map(|s| s.to_string());
        rewrite_artifact_fm("", &fm)
    }

    #[test]
    fn select_dirty_regen_seed_returns_false_when_no_dirty_children() {
        let approved = (
            Uuid::from_bytes([1; 16]),
            artifact_body(ArtifactStatus::Approved, None),
        );
        let pending = (
            Uuid::from_bytes([2; 16]),
            artifact_body(ArtifactStatus::Pending, None),
        );
        let (any_dirty, notes) = select_dirty_regen_seed(&[approved, pending]);
        assert!(!any_dirty);
        assert!(notes.is_none());
    }

    #[test]
    fn select_dirty_regen_seed_picks_up_dirty_child_with_notes() {
        let dirty_id = Uuid::from_bytes([7; 16]);
        let dirty = (
            dirty_id,
            artifact_body(ArtifactStatus::Dirty, Some("Add SLO epic instead")),
        );
        let approved = (
            Uuid::from_bytes([8; 16]),
            artifact_body(ArtifactStatus::Approved, None),
        );
        let (any_dirty, notes) = select_dirty_regen_seed(&[approved, dirty]);
        assert!(any_dirty);
        assert_eq!(
            notes,
            Some((dirty_id, "Add SLO epic instead".to_string()))
        );
    }

    #[test]
    fn select_dirty_regen_seed_reports_dirty_even_when_notes_empty() {
        // A user can mark Dirty without typing any notes (the
        // refinement-notes textarea is optional). The cascade still
        // needs to wipe the child's subtree and rerun the skill —
        // it just won't have notes to inline.
        let dirty = (
            Uuid::from_bytes([9; 16]),
            artifact_body(ArtifactStatus::Dirty, None),
        );
        let (any_dirty, notes) = select_dirty_regen_seed(&[dirty]);
        assert!(any_dirty);
        assert!(notes.is_none());
    }

    #[test]
    fn select_dirty_regen_seed_picks_first_dirty_with_notes() {
        // Two dirty siblings, both with notes. The cascade only has
        // one slot for `extra_revision_notes`; document that we pick
        // the first one. (Order matches the input slice's order — the
        // caller controls determinism via title-sorting upstream.)
        let dirty_a_id = Uuid::from_bytes([10; 16]);
        let dirty_b_id = Uuid::from_bytes([11; 16]);
        let dirty_a = (
            dirty_a_id,
            artifact_body(ArtifactStatus::Dirty, Some("notes A")),
        );
        let dirty_b = (
            dirty_b_id,
            artifact_body(ArtifactStatus::Dirty, Some("notes B")),
        );
        let (any_dirty, notes) = select_dirty_regen_seed(&[dirty_a, dirty_b]);
        assert!(any_dirty);
        assert_eq!(notes, Some((dirty_a_id, "notes A".to_string())));
    }

    #[test]
    fn select_dirty_regen_seed_skips_dirty_without_notes_when_other_has_them() {
        // If multiple siblings are Dirty but only some carry notes,
        // pick the one with notes so the regen prompt actually
        // receives the user's guidance.
        let dirty_no_notes_id = Uuid::from_bytes([12; 16]);
        let dirty_with_notes_id = Uuid::from_bytes([13; 16]);
        let dirty_no_notes = (
            dirty_no_notes_id,
            artifact_body(ArtifactStatus::Dirty, None),
        );
        let dirty_with_notes = (
            dirty_with_notes_id,
            artifact_body(ArtifactStatus::Dirty, Some("emphasize SLOs")),
        );
        let (any_dirty, notes) =
            select_dirty_regen_seed(&[dirty_no_notes, dirty_with_notes]);
        assert!(any_dirty);
        assert_eq!(
            notes,
            Some((dirty_with_notes_id, "emphasize SLOs".to_string()))
        );
    }

    fn skill_with_input(id: &str, input_kind: &str) -> SkillRef {
        let mut c = SkillContract::default();
        c.input_kind = Some(input_kind.to_string());
        SkillRef {
            id: Uuid::new_v4(),
            title: id.to_string(),
            contract: c,
        }
    }

    #[test]
    fn skill_phase_picks_ba_for_master_requirement_root() {
        assert_eq!(
            SkillPhase::for_root_kind(Some("master_requirement")),
            SkillPhase::Ba
        );
    }

    #[test]
    fn skill_phase_picks_ba_for_legacy_requirements_root() {
        assert_eq!(
            SkillPhase::for_root_kind(Some("requirements")),
            SkillPhase::Ba
        );
    }

    #[test]
    fn skill_phase_picks_sde_for_task_root() {
        assert_eq!(SkillPhase::for_root_kind(Some("task")), SkillPhase::Sde);
    }

    #[test]
    fn skill_phase_falls_back_to_mixed_for_unknown_root() {
        assert_eq!(SkillPhase::for_root_kind(None), SkillPhase::Mixed);
        assert_eq!(
            SkillPhase::for_root_kind(Some("plan")),
            SkillPhase::Mixed
        );
    }

    #[test]
    fn filter_skills_for_ba_phase_keeps_ba_chain_and_architecture() {
        let skills = vec![
            skill_with_input("01-ba-agg", "master_requirement"),
            skill_with_input("02-ba-epics", "master_requirement"),
            skill_with_input("03-ba-features", "epic"),
            skill_with_input("06-sa-arch", "master_requirement"),
            skill_with_input("07-sde-impl", "task"),
            skill_with_input("08-sde-tests", "implementation"),
            skill_with_input("10-sde-bug", "bug"),
        ];
        let kept = filter_skills_for_phase(skills, SkillPhase::Ba);
        let titles: Vec<&str> = kept.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["01-ba-agg", "02-ba-epics", "03-ba-features", "06-sa-arch"]
        );
    }

    #[test]
    fn filter_skills_for_sde_phase_drops_ba_chain_and_architecture() {
        let skills = vec![
            skill_with_input("01-ba-agg", "master_requirement"),
            skill_with_input("06-sa-arch", "master_requirement"),
            skill_with_input("07-sde-impl", "task"),
            skill_with_input("08-sde-tests", "implementation"),
            skill_with_input("09-sde-results", "test_cases"),
            skill_with_input("10-sde-bug", "bug"),
        ];
        let kept = filter_skills_for_phase(skills, SkillPhase::Sde);
        let titles: Vec<&str> = kept.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["07-sde-impl", "08-sde-tests", "09-sde-results", "10-sde-bug"]
        );
    }

    #[test]
    fn filter_skills_for_mixed_phase_keeps_everything() {
        let skills = vec![
            skill_with_input("01-ba-agg", "master_requirement"),
            skill_with_input("07-sde-impl", "task"),
            skill_with_input("10-sde-bug", "bug"),
        ];
        let kept = filter_skills_for_phase(skills.clone(), SkillPhase::Mixed);
        assert_eq!(kept.len(), skills.len());
    }

    #[test]
    fn filter_skills_keeps_skills_with_no_input_kind() {
        let mut utility = SkillRef {
            id: Uuid::new_v4(),
            title: "utility-no-kind".into(),
            contract: SkillContract::default(),
        };
        utility.contract.input_kind = None;
        let kept = filter_skills_for_phase(vec![utility.clone()], SkillPhase::Sde);
        assert_eq!(kept.len(), 1);
        let kept_ba = filter_skills_for_phase(vec![utility], SkillPhase::Ba);
        assert_eq!(kept_ba.len(), 1);
    }

    #[test]
    fn requirements_gate_triggers_on_ba_master_requirement_root() {
        assert!(needs_requirements_gate(
            SkillPhase::Ba,
            Some("master_requirement"),
        ));
    }

    #[test]
    fn requirements_gate_skips_legacy_requirements_root() {
        // A `requirements` artifact at the project root is the
        // legacy seed-skills-employee entry. Even though it gets
        // bucketed into the BA phase, the BA-authored-requirements
        // model doesn't apply there — the legacy chain *is* the
        // requirements doc. Skipping prevents a false-positive
        // cascade halt.
        assert!(!needs_requirements_gate(
            SkillPhase::Ba,
            Some("requirements"),
        ));
    }

    #[test]
    fn requirements_gate_skips_sde_phase() {
        // Per-task Play (SDE phase) has nothing to do with
        // requirements authoring — the BA tree should already exist.
        assert!(!needs_requirements_gate(
            SkillPhase::Sde,
            Some("task"),
        ));
    }

    #[test]
    fn requirements_gate_skips_mixed_phase_and_unknown_roots() {
        assert!(!needs_requirements_gate(SkillPhase::Mixed, Some("plan")));
        assert!(!needs_requirements_gate(SkillPhase::Mixed, None));
        assert!(!needs_requirements_gate(SkillPhase::Ba, None));
    }

    #[test]
    fn empty_requirements_message_mentions_artifact_kind_and_status() {
        // The error message is surfaced verbatim in the
        // cascade-status row — verify it tells the BA what
        // frontmatter fields to set so the fix is actionable
        // without consulting docs.
        let msg = empty_requirements_message();
        assert!(msg.contains("artifact_kind: requirements"));
        assert!(msg.contains("status: approved"));
        assert!(msg.contains("Add child note"));
    }
}
