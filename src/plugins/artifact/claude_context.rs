//! Compose `<repo>/.claude/CLAUDE.md` so Claude Code's auto-loaded
//! project-context surface has Operon's SDLC schema *and* the current
//! artifact inventory. Claude reads CLAUDE.md once at session start;
//! Operon regenerates this file at cascade kickoff so the freshest
//! state lands in front of the first node's prompt.
//!
//! The composer is split into a pure [`compose`] that takes
//! pre-loaded notes + their parsed artifact kinds, and an async
//! [`write_project_claude_md`] wrapper that loads bodies via the
//! [`Persistence`] layer. The split keeps the composition rule
//! unit-testable without spinning up a fake repo.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use operon_store::repos::{LocalNote, LocalNoteRepository, NoteKind};
use uuid::Uuid;

use crate::persistence::{PersistError, Persistence};
use crate::plugins::artifact::frontmatter::{self, ArtifactKind};

/// File Operon writes inside the project's repository so the Claude
/// CLI auto-loads it as session context.
const TARGET_FILENAME: &str = "CLAUDE.md";

/// One entry in the artifact inventory section — a (kind, title) pair
/// rendered as a bullet under its kind's heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEntry {
    pub id: Uuid,
    pub title: String,
    pub kind: ArtifactKind,
}

/// Pure composer: given the project's display name and a flat list of
/// parsed artifact entries, return the markdown that Operon writes to
/// `<repo>/.claude/CLAUDE.md`. No I/O.
///
/// Entries are grouped by `artifact_kind` in waterfall order, then
/// sorted alphabetically by title within each group so the output is
/// deterministic — regenerating the file with the same inputs
/// produces byte-identical content.
pub fn compose(project_name: &str, entries: &[ArtifactEntry]) -> String {
    let mut by_kind: BTreeMap<String, Vec<&ArtifactEntry>> = BTreeMap::new();
    for entry in entries {
        by_kind
            .entry(entry.kind.as_str().to_string())
            .or_default()
            .push(entry);
    }
    for group in by_kind.values_mut() {
        group.sort_by(|a, b| a.title.cmp(&b.title));
    }

    let mut out = String::new();
    out.push_str(
        "# Operon SDLC context\n\
         \n\
         This project uses the Operon SDLC pipeline. Every artifact is a\n\
         markdown note whose YAML frontmatter declares an `artifact_kind`\n\
         and links to a typed parent through Operon's note tree. Operon\n\
         enforces the parent-kind contract: a `feature` artifact must\n\
         descend from an `epic`, a `story` from a `feature`, and so on.\n\
         \n\
         Skills live in `.claude/skills/` and declare `input_kind` /\n\
         `output_kind` in their YAML frontmatter so each skill applies\n\
         to a specific column of the waterfall.\n\
         \n\
         ## Kind taxonomy (waterfall depth)\n\
         \n\
         | Depth | Kind | Role |\n\
         |---|---|---|\n\
         | 0 | `master_requirement` | Project root: charter + CE-team inputs |\n\
         | 1 | `requirements` | Decomposed needs derived from a master_requirement |\n\
         | 1 | `epic` | Large feature area derived from a master_requirement |\n\
         | 1 | `architecture` | Technical architecture; sibling to epics |\n\
         | 2 | `feature` | Discrete capability decomposed from an epic |\n\
         | 3 | `story` | User-facing story under a feature |\n\
         | 4 | `task` | Implementation unit under a story |\n\
         | 5 | `plan` / `implementation_plan` | Implementation plan for a task |\n\
         | 6 | `implementation` | Code change produced from a plan |\n\
         | 6 | `test_cases` / `test_results` / `summary` | Verification and rollup |\n\
         \n",
    );

    out.push_str(&format!("## Current project: {project_name}\n\n"));

    if entries.is_empty() {
        out.push_str("_No artifacts yet — the cascade has not produced anything for this project._\n");
        return out;
    }

    out.push_str(&format!("Total artifacts: **{}**.\n\n", entries.len()));
    out.push_str("### Inventory by kind\n\n");

    for kind in canonical_kind_order() {
        let Some(group) = by_kind.remove(*kind) else { continue };
        out.push_str(&format!("- **{}** ({})\n", kind, group.len()));
        for entry in group.iter().take(MAX_ENTRIES_PER_KIND) {
            let short_id = short_uuid(&entry.id);
            out.push_str(&format!("    - {} _(id: {short_id})_\n", entry.title));
        }
        if group.len() > MAX_ENTRIES_PER_KIND {
            let extra = group.len() - MAX_ENTRIES_PER_KIND;
            out.push_str(&format!("    - _(\u{2026}{extra} more)_\n"));
        }
    }
    // Any kinds not in the canonical order (e.g. `Other(...)`) ride
    // through at the end so custom artifact kinds still surface.
    for (kind, group) in by_kind {
        out.push_str(&format!("- **{}** ({})\n", kind, group.len()));
        for entry in group.iter().take(MAX_ENTRIES_PER_KIND) {
            let short_id = short_uuid(&entry.id);
            out.push_str(&format!("    - {} _(id: {short_id})_\n", entry.title));
        }
        if group.len() > MAX_ENTRIES_PER_KIND {
            let extra = group.len() - MAX_ENTRIES_PER_KIND;
            out.push_str(&format!("    - _(\u{2026}{extra} more)_\n"));
        }
    }

    out.push_str(
        "\n---\n\
         _Generated by Operon. Regenerated on every cascade kickoff;\n\
         do not edit by hand._\n",
    );
    out
}

/// Cap on artifact entries shown per kind, to keep CLAUDE.md small
/// even when a cascade has produced dozens of leaf tasks. Excess
/// entries are summarised as `(…N more)`.
const MAX_ENTRIES_PER_KIND: usize = 12;

/// Waterfall-depth order used by the inventory. Mirrors the taxonomy
/// table at the top of the file so the reader's eye moves left-to-
/// right through the same column order.
fn canonical_kind_order() -> &'static [&'static str] {
    &[
        "master_requirement",
        "requirements",
        "epic",
        "architecture",
        "architecture_review",
        "feature",
        "story",
        "task",
        "plan",
        "implementation_plan",
        "implementation",
        "test_cases",
        "test_results",
        "summary",
        "bug",
        "clarification",
        "prioritized_backlog",
    ]
}

fn short_uuid(id: &Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

/// I/O wrapper: load every artifact note's body to discover its
/// `artifact_kind`, then write the composed CLAUDE.md to
/// `<repo>/.claude/CLAUDE.md`. Returns the absolute path on success.
///
/// Errors that affect a single artifact's body load are swallowed
/// (logged) rather than aborting the whole compose — a malformed
/// frontmatter on one note shouldn't kill Claude's view of the
/// other 49.
pub async fn write_project_claude_md(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    project_name: &str,
    repo_path: &Path,
) -> Result<PathBuf, io::Error> {
    let notes = note_repo
        .list_for_project(project_id)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("list_for_project: {e}")))?;

    let mut entries: Vec<ArtifactEntry> = Vec::new();
    for note in &notes {
        if !matches!(note.kind, NoteKind::Artifact) {
            continue;
        }
        let kind = match resolve_artifact_kind(persistence.as_ref(), note).await {
            Some(k) => k,
            None => continue,
        };
        entries.push(ArtifactEntry {
            id: note.id,
            title: note.title.clone(),
            kind,
        });
    }

    let body = compose(project_name, &entries);
    let dir = repo_path.join(".claude");
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(TARGET_FILENAME);
    std::fs::write(&target, body)?;
    Ok(target)
}

async fn resolve_artifact_kind(
    persistence: &dyn Persistence,
    note: &LocalNote,
) -> Option<ArtifactKind> {
    let id = note.id.to_string();
    let bytes = match persistence.load(&id).await {
        Ok(b) => b,
        Err(PersistError::NotFound) => return None,
        Err(e) => {
            tracing::warn!(
                target: "operon::claude_context",
                "load body for artifact {id}: {e:?}"
            );
            return None;
        }
    };
    let body = String::from_utf8(bytes).ok()?;
    frontmatter::parse(&body).artifact_kind
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, kind: ArtifactKind) -> ArtifactEntry {
        ArtifactEntry {
            id: Uuid::new_v4(),
            title: title.into(),
            kind,
        }
    }

    #[test]
    fn compose_empty_project_emits_placeholder() {
        let out = compose("My Project", &[]);
        assert!(out.contains("# Operon SDLC context"));
        assert!(out.contains("My Project"));
        assert!(out.contains("No artifacts yet"));
    }

    #[test]
    fn compose_groups_entries_by_kind_in_waterfall_order() {
        let entries = vec![
            entry("Feature A", ArtifactKind::Feature),
            entry("Root", ArtifactKind::MasterRequirement),
            entry("Epic X", ArtifactKind::Epic),
            entry("Story 1", ArtifactKind::Story),
        ];
        let out = compose("P", &entries);
        // Look at the bolded inventory headings (`**kind**`), not the
        // bare kind names — those appear earlier in the taxonomy table.
        let root = out.find("**master_requirement**").unwrap();
        let epic = out.find("**epic**").unwrap();
        let feature = out.find("**feature**").unwrap();
        let story = out.find("**story**").unwrap();
        assert!(root < epic && epic < feature && feature < story);
    }

    #[test]
    fn compose_caps_entries_per_kind() {
        let mut entries = Vec::new();
        for i in 0..20 {
            entries.push(entry(&format!("Task {i:02}"), ArtifactKind::Task));
        }
        let out = compose("P", &entries);
        // 20 total, MAX_ENTRIES_PER_KIND shown, so 8 hidden.
        assert!(out.contains("\u{2026}8 more"));
        // Cap respected: only 12 bullet entries for `task`.
        let task_bullets = out
            .lines()
            .filter(|l| l.trim_start().starts_with("- Task "))
            .count();
        assert_eq!(task_bullets, MAX_ENTRIES_PER_KIND);
    }

    #[test]
    fn compose_is_deterministic() {
        let entries = vec![
            entry("Zebra", ArtifactKind::Epic),
            entry("Alpha", ArtifactKind::Epic),
            entry("Mango", ArtifactKind::Epic),
        ];
        let a = compose("P", &entries);
        let b = compose("P", &entries);
        assert_eq!(a, b);
        // Alphabetical order within a kind.
        let alpha = a.find("Alpha").unwrap();
        let mango = a.find("Mango").unwrap();
        let zebra = a.find("Zebra").unwrap();
        assert!(alpha < mango && mango < zebra);
    }

    #[test]
    fn compose_surfaces_other_kinds_after_canonical() {
        let entries = vec![
            entry("Standard Epic", ArtifactKind::Epic),
            entry("Custom Thing", ArtifactKind::Other("custom_kind".into())),
        ];
        let out = compose("P", &entries);
        // Custom kind appears in the inventory but after the canonical
        // ordering.
        let epic_pos = out.find("**epic**").unwrap();
        let custom_pos = out.find("**custom_kind**").unwrap();
        assert!(epic_pos < custom_pos);
    }

    #[test]
    fn compose_includes_kind_taxonomy_table() {
        let out = compose("P", &[]);
        assert!(out.contains("Kind taxonomy"));
        assert!(out.contains("master_requirement"));
        assert!(out.contains("epic"));
        assert!(out.contains("feature"));
        assert!(out.contains("task"));
    }
}
