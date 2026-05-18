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
#[derive(Debug, Default, Clone)]
pub struct SkillInstallReport {
    /// New skills inserted during this pass.
    pub installed: usize,
    /// Existing skills (by exact title match) skipped to keep the call
    /// idempotent across repeated installs.
    pub skipped: usize,
    /// Skills the install attempted but could not persist.
    pub failed: usize,
    /// Human-readable error messages, one per failed skill (plus a
    /// SKILLS-index entry if that save failed). Empty on the happy
    /// path. Capped at MAX_REPORT_ERRORS so a wholesale failure doesn't
    /// generate a huge MCP payload. Same strings are also logged via
    /// `eprintln!` for stderr trace.
    pub errors: Vec<String>,
}

/// Hard cap on `SkillInstallReport.errors` length so a total install
/// failure (e.g. notes_dir unwritable) doesn't pump a 15-line error
/// vector through every MCP response.
pub const MAX_REPORT_ERRORS: usize = 20;

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
                    let msg = format!("{}: save body: {e}", skill.stem);
                    eprintln!("operon: install_skills {msg}");
                    push_capped_error(&mut report.errors, msg);
                    report.failed += 1;
                    continue;
                }
                report.installed += 1;
            }
            Err(e) => {
                let msg = format!("{}: create_with_kind: {e}", skill.stem);
                eprintln!("operon: install_skills {msg}");
                push_capped_error(&mut report.errors, msg);
                report.failed += 1;
            }
        }
    }

    let body = build_skills_index_body(note_repo, project_id, skills_parent_id, readme);
    if let Err(e) = persistence
        .save(&skills_parent_id.to_string(), body.as_bytes())
        .await
    {
        // The skill rows are already in place — bodies, where saved,
        // are still on disk. Bump `failed` and surface the error so
        // the caller (and the LLM) knows the SKILLS index will be
        // stale until the next install pass.
        let msg = format!("SKILLS-index: save body: {e}");
        eprintln!("operon: install_skills {msg}");
        push_capped_error(&mut report.errors, msg);
        report.failed += 1;
    }

    Ok(report)
}

/// Append `msg` to `errors` if there's room; otherwise drop it silently
/// (the eprintln! at the call site preserves the full trace on stderr).
/// Keeps MCP payloads bounded even when an entire install fails.
fn push_capped_error(errors: &mut Vec<String>, msg: String) {
    if errors.len() < MAX_REPORT_ERRORS {
        errors.push(msg);
    }
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
                ("03-decompose-stories".into(), id_b),
            ],
        );
        assert!(body.contains("# SKILLS"));
        assert!(body.contains("## Imported skills"));
        assert!(body.contains(&format!("- [02-discover-epics](operon://note/{id_a})")));
        assert!(body.contains(&format!("- [03-decompose-stories](operon://note/{id_b})")));
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

    /// `Persistence` impl whose `save` always errors. Used to verify
    /// that `install_skills_into_project` collects the error string in
    /// `report.errors` rather than swallowing it via `eprintln!`.
    struct AlwaysFailSave;

    impl crate::persistence::Persistence for AlwaysFailSave {
        fn load<'a>(
            &'a self,
            _note_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Vec<u8>, crate::persistence::PersistError>,
                    > + 'a,
            >,
        > {
            Box::pin(async { Err(crate::persistence::PersistError::NotFound) })
        }

        fn save<'a>(
            &'a self,
            _note_id: &'a str,
            _bytes: &'a [u8],
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::persistence::PersistError>,
                    > + 'a,
            >,
        > {
            Box::pin(async {
                Err(crate::persistence::PersistError::Io(
                    "simulated disk failure".into(),
                ))
            })
        }

        fn list<'a>(
            &'a self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::persistence::NoteRef>,
                            crate::persistence::PersistError,
                        >,
                    > + 'a,
            >,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn delete<'a>(
            &'a self,
            _note_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::persistence::PersistError>,
                    > + 'a,
            >,
        > {
            Box::pin(async { Ok(()) })
        }

        fn rename<'a>(
            &'a self,
            _from: &'a str,
            _to: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::persistence::PersistError>,
                    > + 'a,
            >,
        > {
            Box::pin(async { Ok(()) })
        }
    }

    /// Build an in-memory project + repo bundle for install_skills
    /// tests. Returns (note_repo, persistence, project_id) wired
    /// against an in-memory SQLite store + `MemoryPersistence`.
    fn install_test_bed() -> (
        Arc<dyn LocalNoteRepository>,
        Arc<dyn crate::persistence::Persistence>,
        Uuid,
    ) {
        use operon_store::repos::{
            LocalProjectRepository, SqliteLocalNoteRepository, SqliteLocalProjectRepository,
        };
        use operon_store::Store;
        let store = Store::for_test().expect("in-memory store");
        let note_repo: Arc<dyn LocalNoteRepository> =
            Arc::new(SqliteLocalNoteRepository::new(store.clone()));
        let project_repo = SqliteLocalProjectRepository::new(store.clone());
        let project = project_repo.create("test").expect("create project");
        let persistence: Arc<dyn crate::persistence::Persistence> =
            Arc::new(crate::persistence::MemoryPersistence::new());
        (note_repo, persistence, project.id)
    }

    #[tokio::test]
    async fn install_skills_surfaces_save_errors_in_report() {
        // Wire a real (sqlite) note repo against a failing persistence
        // so create_with_kind succeeds (notes appear in the tree) but
        // every body save errors — exactly the empty-skills failure
        // mode the user reported via MCP.
        let (note_repo, _ok_persistence, project_id) = install_test_bed();
        let failing: Arc<dyn crate::persistence::Persistence> = Arc::new(AlwaysFailSave);

        let skills = vec![
            SkillSource { stem: "01-foo", body: "## one" },
            SkillSource { stem: "02-bar", body: "## two" },
        ];
        let report =
            install_skills_into_project(&note_repo, &failing, project_id, skills, None)
                .await
                .expect("install returns a report even when saves fail");

        assert_eq!(report.installed, 0, "no skill body was successfully saved");
        // 2 per-skill failures + 1 SKILLS-index failure = 3 total.
        assert_eq!(report.failed, 3);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("simulated disk failure")),
            "expected PersistError message to surface in report.errors; got {:?}",
            report.errors,
        );
        assert!(
            report.errors.iter().any(|e| e.starts_with("01-foo:")),
            "expected per-skill stem prefix in error string; got {:?}",
            report.errors,
        );
    }

    #[tokio::test]
    async fn install_skills_happy_path_persists_non_empty_bodies() {
        // The bug we're guarding against: skill notes are created in
        // the tree but their bodies never land on disk. This test
        // installs two synthetic skills against a working
        // MemoryPersistence and asserts the bodies are loadable AND
        // non-empty AND match what was passed in.
        let (note_repo, persistence, project_id) = install_test_bed();

        let skills = vec![
            SkillSource { stem: "01-foo", body: "BODY ONE" },
            SkillSource { stem: "02-bar", body: "BODY TWO" },
        ];
        let report = install_skills_into_project(
            &note_repo,
            &persistence,
            project_id,
            skills,
            Some("# Pipeline\n"),
        )
        .await
        .expect("happy-path install");

        assert_eq!(report.installed, 2);
        assert_eq!(report.failed, 0);
        assert!(report.errors.is_empty(), "no errors expected on happy path");

        // Walk the created Skill notes and verify each body round-trips.
        let notes = note_repo.list_for_project(project_id).unwrap();
        let skill_rows: Vec<_> = notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::Skill))
            .collect();
        assert_eq!(skill_rows.len(), 2);
        for row in skill_rows {
            let bytes = persistence
                .load(&row.id.to_string())
                .await
                .expect("body should be persisted");
            let body = String::from_utf8(bytes).unwrap();
            assert!(
                !body.is_empty(),
                "{} should have a non-empty body",
                row.title
            );
            let expected = if row.title == "01-foo" {
                "BODY ONE"
            } else {
                "BODY TWO"
            };
            assert_eq!(body, expected, "body for {} mismatched", row.title);
        }
    }
}
