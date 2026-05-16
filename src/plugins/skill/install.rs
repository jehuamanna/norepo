//! Shared "install skills into a project" core.
//!
//! Both the explorer's "Import skills…" folder-picker flow
//! (`src/local_mode/explorer/project_row.rs::import_skills_from_folder`)
//! and the `install_seed_skills` MCP tool funnel through
//! [`install_skills_into_project`] so the SKILLS-index + idempotency +
//! cascade-ordered insertion logic lives in exactly one place. The two
//! callers differ only in how they source the (stem, body) pairs:
//! folder picker reads from disk, MCP tool pulls from the embedded
//! [`super::seed::SEED_SKILLS`] bundle.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashSet;
use std::sync::Arc;

use operon_store::repos::{LocalNoteRepository, NoteKind};
use uuid::Uuid;

use crate::persistence::Persistence;

/// Title used for the auto-managed skill index note. Stored at project
/// root with `NoteKind::Markdown`. Find-or-create matches on exact
/// title + kind + root-level position so a renamed / moved index gets a
/// fresh one rather than colliding.
pub const SKILLS_PARENT_TITLE: &str = "SKILLS";

/// One skill to install, identified by its title-stem and full body.
pub struct SkillSource<'a> {
    /// Title-stem (e.g. `02-ba-discover-epics`). Becomes both the
    /// `local_note.title` and the on-disk slug if/when this skill is
    /// later materialized to `<repo>/.claude/skills/<stem>.md`.
    pub stem: &'a str,
    /// Raw body, including any YAML frontmatter the skill ships with.
    pub body: &'a str,
}

/// Outcome of a single install pass — counts for the caller to surface.
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillInstallReport {
    /// New skills inserted during this pass.
    pub installed: usize,
    /// Existing skills (by exact title match) skipped to keep the call
    /// idempotent across repeated installs.
    pub skipped: usize,
    /// Skills the install attempted but could not persist (logged via
    /// `eprintln!`; surfaces here so the caller can flag a partial run).
    pub failed: usize,
}

/// Install a batch of skills into a project under the `SKILLS` index
/// note. Idempotent on title: pre-existing skills with the same title
/// are left alone and counted in [`SkillInstallReport::skipped`].
///
/// `readme` becomes the prose preamble of the SKILLS index note. When
/// `None`, a minimal `# SKILLS\n` header is used instead. The index
/// body is regenerated on every call so it stays in sync with the
/// current skill children.
pub async fn install_skills_into_project<'a, I>(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
    skills: I,
    readme: Option<&str>,
) -> Result<SkillInstallReport, String>
where
    I: IntoIterator<Item = SkillSource<'a>>,
{
    let skills_parent_id = find_or_create_skills_parent(note_repo, persistence, project_id)
        .await
        .ok_or_else(|| "failed to find or create SKILLS parent note".to_string())?;

    let existing_titles: HashSet<String> = note_repo
        .list_for_project(project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| matches!(n.kind, NoteKind::Skill))
        .map(|n| n.title)
        .collect();

    let mut report = SkillInstallReport::default();
    for skill in skills {
        if existing_titles.contains(skill.stem) {
            report.skipped += 1;
            continue;
        }
        match note_repo.create_with_kind(
            project_id,
            Some(skills_parent_id),
            skill.stem,
            NoteKind::Skill,
        ) {
            Ok(row) => {
                if let Err(e) = persistence
                    .save(&row.id.to_string(), skill.body.as_bytes())
                    .await
                {
                    eprintln!(
                        "operon: install_skills save failed for {}: {e}",
                        skill.stem
                    );
                    report.failed += 1;
                    continue;
                }
                report.installed += 1;
            }
            Err(e) => {
                eprintln!(
                    "operon: install_skills create_with_kind failed for {}: {e}",
                    skill.stem
                );
                report.failed += 1;
            }
        }
    }

    let body = build_skills_index_body(note_repo, project_id, skills_parent_id, readme);
    if let Err(e) = persistence
        .save(&skills_parent_id.to_string(), body.as_bytes())
        .await
    {
        // Non-fatal: the skill rows are already in place. The index
        // note will be stale until the next install run rewrites it.
        eprintln!("operon: install_skills SKILLS-index save failed: {e}");
    }

    Ok(report)
}

/// Find the project's `SKILLS` index note (root-level Markdown note
/// titled `SKILLS`), or create one if absent. Returns its id, or
/// `None` if the repo lookup / creation failed.
pub async fn find_or_create_skills_parent(
    note_repo: &Arc<dyn LocalNoteRepository>,
    persistence: &Arc<dyn Persistence>,
    project_id: Uuid,
) -> Option<Uuid> {
    let all = note_repo.list_for_project(project_id).ok()?;
    if let Some(existing) = all.iter().find(|n| {
        n.parent_id.is_none()
            && n.title == SKILLS_PARENT_TITLE
            && matches!(n.kind, NoteKind::Markdown)
    }) {
        return Some(existing.id);
    }
    let row = note_repo
        .create_with_kind(project_id, None, SKILLS_PARENT_TITLE, NoteKind::Markdown)
        .ok()?;
    // Seed an empty body — `install_skills_into_project` rewrites the
    // body at the end of every install with the README + auto-list.
    // The seed exists only so opening the note before the first install
    // shows something rather than an empty file.
    let _ = persistence
        .save(&row.id.to_string(), b"# SKILLS\n")
        .await;
    Some(row.id)
}

/// Build the SKILLS index body. Composition: optional README prose +
/// an auto-generated `## Imported skills` section listing every skill
/// child as an `operon://note/<uuid>` markdown link the renderer wires
/// up to navigation.
pub fn build_skills_index_body(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_id: Uuid,
    skills_parent_id: Uuid,
    readme: Option<&str>,
) -> String {
    let all = note_repo.list_for_project(project_id).unwrap_or_default();
    let mut children: Vec<(String, Uuid)> = all
        .into_iter()
        .filter(|n| {
            n.parent_id == Some(skills_parent_id) && matches!(n.kind, NoteKind::Skill)
        })
        .map(|n| (n.title, n.id))
        .collect();
    children.sort_by(|a, b| a.0.cmp(&b.0));
    render_skills_index_body(readme, &children)
}

/// Pure renderer for the SKILLS index body. Split from
/// `build_skills_index_body` so the formatting is unit-testable
/// without a repo.
pub fn render_skills_index_body(readme: Option<&str>, children: &[(String, Uuid)]) -> String {
    let mut out = match readme {
        Some(r) => {
            let trimmed = r.trim_end();
            if trimmed.is_empty() {
                String::from("# SKILLS\n")
            } else {
                let mut s = String::with_capacity(trimmed.len() + 64);
                s.push_str(trimmed);
                s.push('\n');
                s
            }
        }
        None => String::from("# SKILLS\n"),
    };
    out.push_str("\n## Imported skills\n\n");
    if children.is_empty() {
        out.push_str("_(no skills imported yet — re-run \"Import skills…\" to add some)_\n");
    } else {
        for (title, id) in children {
            out.push_str(&format!("- [{title}](operon://note/{id})\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_index_lists_children_as_operon_links() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let body = render_skills_index_body(
            None,
            &[
                ("02-discover-epics".into(), id_a),
                ("03-decompose-features".into(), id_b),
            ],
        );
        assert!(body.contains("# SKILLS"));
        assert!(body.contains("## Imported skills"));
        assert!(body.contains(&format!("- [02-discover-epics](operon://note/{id_a})")));
        assert!(body.contains(&format!("- [03-decompose-features](operon://note/{id_b})")));
    }

    #[test]
    fn skills_index_uses_readme_as_preamble_when_present() {
        let readme = "# Seed skills\n\nThis chain decomposes requirements into tasks.\n";
        let body = render_skills_index_body(Some(readme), &[]);
        assert!(body.starts_with("# Seed skills\n"));
        assert!(body.contains("This chain decomposes requirements"));
        assert!(body.contains("## Imported skills"));
        assert!(body.contains("_(no skills imported yet"));
    }

    #[test]
    fn skills_index_falls_back_to_default_header_when_readme_blank() {
        let body = render_skills_index_body(Some("   \n\n  "), &[]);
        assert!(body.starts_with("# SKILLS\n"));
        assert!(body.contains("## Imported skills"));
    }

    #[test]
    fn skills_index_separates_readme_from_auto_section() {
        let id = Uuid::new_v4();
        let body = render_skills_index_body(
            Some("# Pipeline\n\nNarrative ends here."),
            &[("02-discover-epics".into(), id)],
        );
        assert!(body.contains("Narrative ends here.\n\n## Imported skills"));
    }
}
