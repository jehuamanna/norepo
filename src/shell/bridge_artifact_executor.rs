//! Host-side implementation of [`ArtifactExecutor`] (M4): turn a
//! `mcp__operon__create_artifact` tool call from Claude into a real
//! `NoteKind::Artifact` row in Operon's project tree.
//!
//! Why this exists: the legacy contract between Operon and Claude has
//! Claude write `.md` files into a per-cascade scratch directory, and
//! Operon scan-imports them after the run completes. The kind +
//! parent are derived heuristically (frontmatter parsing + ancestor
//! walk), which means a structural mistake by the model is only
//! caught post-hoc — see commit `3374731` for the most recent
//! re-parenting fixup.
//!
//! `create_artifact` flips that: the kind and parent are typed tool-
//! call arguments. The host validates them against the live note
//! tree at the moment of the call and either creates the artifact
//! correctly or rejects the call with a message Claude can read.
//! No mtime scan, no heuristic re-parenting.
//!
//! Lifecycle: one [`BridgeArtifactExecutor`] is constructed per
//! session in [`crate::shell::companion_state::ensure_session_bridge`]
//! after the project context is resolved from `cwd`. It carries
//! `Arc`-clones of [`LocalNoteRepository`] and [`Persistence`] plus
//! the resolved `project_id`. Dropping the bridge drops the executor.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use futures::future::LocalBoxFuture;
use operon_core::error::OperonError;
use operon_core::error::OperonResult;
use operon_plugins_claude_code::ArtifactExecutor;
use operon_store::repos::{LocalNoteRepository, NoteKind};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::plugins::artifact::frontmatter::{
    parse as parse_artifact_fm, rewrite as rewrite_artifact_fm, ArtifactKind, ArtifactStatus,
};
use crate::plugins::artifact::revision_table;

pub struct BridgeArtifactExecutor {
    note_repo: Arc<dyn LocalNoteRepository>,
    persistence: Arc<dyn Persistence>,
    project_id: Uuid,
}

impl BridgeArtifactExecutor {
    pub fn new(
        note_repo: Arc<dyn LocalNoteRepository>,
        persistence: Arc<dyn Persistence>,
        project_id: Uuid,
    ) -> Self {
        Self {
            note_repo,
            persistence,
            project_id,
        }
    }
}

impl ArtifactExecutor for BridgeArtifactExecutor {
    fn create<'a>(&'a self, args: Value) -> LocalBoxFuture<'a, OperonResult<Value>> {
        let note_repo = self.note_repo.clone();
        let persistence = self.persistence.clone();
        let project_id = self.project_id;
        Box::pin(async move { create_inner(note_repo, persistence, project_id, args).await })
    }
}

/// Wrap a string message as `OperonError::Plugin` keyed to this
/// executor — keeps the call sites below readable. The variant
/// wants a boxed std::error::Error source, so we adapt via
/// `std::io::Error::other`.
fn err(message: impl Into<String>) -> OperonError {
    OperonError::Plugin {
        plugin: "create_artifact".into(),
        source: Box::new(std::io::Error::other(message.into())),
    }
}

async fn create_inner(
    note_repo: Arc<dyn LocalNoteRepository>,
    persistence: Arc<dyn Persistence>,
    project_id: Uuid,
    args: Value,
) -> OperonResult<Value> {
    let kind_str = require_string(&args, "kind")?;
    let parent_str = require_string(&args, "parent_id")?;
    let title = require_string(&args, "title")?;
    let body = require_string(&args, "body")?;

    let kind = ArtifactKind::parse(&kind_str);
    let parent_id = Uuid::parse_str(&parent_str)
        .map_err(|e| err(format!("parent_id is not a valid UUID: {e}")))?;

    // Validate the parent exists *and* belongs to this project so a
    // confused model can't graft an artifact into the wrong project.
    let parent_project = note_repo
        .find_project_for_note(parent_id)
        .map_err(|e| err(format!("find parent project: {e}")))?;
    match parent_project {
        Some(pid) if pid == project_id => {}
        Some(other) => {
            return Err(err(format!(
                "parent_id {parent_id} lives in project {other}, not the active project {project_id}"
            )));
        }
        None => {
            return Err(err(format!("parent_id {parent_id} does not exist")));
        }
    }

    let note = note_repo
        .create_with_kind(project_id, Some(parent_id), &title, NoteKind::Artifact)
        .map_err(|e| err(format!("create note: {e}")))?;

    let normalized_body = normalize_body(&body, &kind);
    let seeded_body = seed_revision_history(&normalized_body, &title);

    persistence
        .save(&note.id.to_string(), seeded_body.as_bytes())
        .await
        .map_err(|e| err(format!("save body: {e:?}")))?;

    // Bump the explorer's reactivity counter so the new note appears
    // in the tree without a manual refresh.
    crate::shell::companion_state::LOCAL_NOTE_VERSION
        .with_mut(|v| *v = v.saturating_add(1));

    Ok(json!({
        "id": note.id.to_string(),
        "title": title,
        "kind": kind.as_str(),
        "parent_id": parent_id.to_string(),
    }))
}

fn require_string(args: &Value, key: &str) -> OperonResult<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| err(format!("missing string arg `{key}`")))
}

/// Strip whatever revision-history table the caller put in `body` and
/// seed a canonical row 1 with `derived_from = "claude"`. The model
/// often copies the explorer-scaffold pattern (which uses `manual
/// entry`) into the body it provides, which would misattribute the
/// creation. Owning this column from the host side guarantees correct
/// provenance regardless of what the model wrote.
fn seed_revision_history(body: &str, title: &str) -> String {
    let stripped = revision_table::strip_revision_section(body);
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let row = revision_table::RevisionRow {
        revision: 1,
        date: revision_table::format_revision_date(unix_ms),
        derived_from: "claude".to_string(),
        summary: format!("Created '{title}' via Claude Code."),
    };
    revision_table::append_revision_row(&stripped, row)
}

/// Normalize the caller-supplied body so its YAML frontmatter declares
/// the typed `artifact_kind` we were called with — even if the model
/// forgot it, used a different value, or sent the body without any
/// frontmatter at all. Preserves any other fields the caller put on
/// the block.
fn normalize_body(body: &str, kind: &ArtifactKind) -> String {
    let mut fm = parse_artifact_fm(body);
    fm.artifact_kind = Some(kind.clone());
    if matches!(fm.status, ArtifactStatus::Pending) {
        // `parse` falls back to Pending when status is missing, which
        // is exactly what we want for a fresh skill output. Leaving
        // this explicit so the caller's `status: approved` on a body
        // they hand-edited rides through.
    }
    rewrite_artifact_fm(body, &fm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::artifact::frontmatter::ArtifactKind;

    #[test]
    fn normalize_body_injects_artifact_kind_when_absent() {
        let body = "# Hello\n\nbody prose";
        let out = normalize_body(body, &ArtifactKind::Epic);
        assert!(out.contains("artifact_kind: epic"));
        assert!(out.contains("body prose"));
    }

    #[test]
    fn normalize_body_overrides_mismatched_kind() {
        // Model claimed "feature" in frontmatter but the typed tool
        // call says "epic" — typed args win.
        let body = "---\nartifact_kind: feature\n---\n\nbody";
        let out = normalize_body(body, &ArtifactKind::Epic);
        assert!(out.contains("artifact_kind: epic"));
        assert!(!out.contains("artifact_kind: feature"));
    }

    #[test]
    fn require_string_rejects_missing_field() {
        let v = json!({});
        let result = require_string(&v, "title");
        assert!(matches!(result, Err(OperonError::Plugin { .. })));
    }

    #[test]
    fn require_string_extracts_value() {
        let v = json!({ "title": "hello" });
        let s = require_string(&v, "title").unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn seed_revision_history_replaces_model_supplied_manual_row() {
        // The model often parrots the explorer scaffold's `manual entry`
        // row into bodies it generates for `create_artifact`. The seed
        // pass must drop that and stamp a single `claude` row 1.
        let body = "---\nartifact_kind: epic\n---\n\n# Epic: Memory Match\n\n\
                    ## Revision history\n\n\
                    | Revision | Date | Derived from | Summary |\n\
                    |-|-|-|-|\n\
                    | 1 | 2026-05-15 | manual entry | Manually created via Operon explorer. |\n";
        let out = super::seed_revision_history(body, "Memory Match");
        let parsed = crate::plugins::artifact::revision_table::parse_revision_table(&out)
            .expect("seeded body has a revision table");
        assert_eq!(parsed.rows.len(), 1, "exactly one row after seeding");
        assert_eq!(parsed.rows[0].derived_from, "claude");
        assert!(
            !out.contains("manual entry"),
            "model's `manual entry` row must be discarded; got:\n{out}"
        );
        assert!(parsed.rows[0].summary.contains("Memory Match"));
    }

    #[test]
    fn seed_revision_history_adds_row_when_body_has_no_table() {
        let body = "---\nartifact_kind: epic\n---\n\n# Epic: \n\n## Outcome\n\n";
        let out = super::seed_revision_history(body, "Untitled");
        let parsed = crate::plugins::artifact::revision_table::parse_revision_table(&out)
            .expect("seeded body has a revision table");
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].derived_from, "claude");
    }
}
