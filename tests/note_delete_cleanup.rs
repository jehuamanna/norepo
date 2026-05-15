//! Integration tests for `plugins::cleanup::note_delete` — verifies that
//! deleting a note cascades into the disk side-effects every plugin
//! produces (skill materializations, artifact dirs, image blobs).

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::Arc;

use operon_store::repos::{
    LocalNoteRepository, LocalProjectRepository, NoteKind, SqliteLocalNoteRepository,
    SqliteLocalProjectRepository,
};
use operon_store::Store;

use operon_dioxus::local_mode::vault::VaultRoot;
use operon_dioxus::persistence::{FilesystemPersistence, Persistence};
use operon_dioxus::plugins::artifact::relocate::RelocatingNoteRepo;
use operon_dioxus::plugins::cleanup::note_delete::delete_note_with_disk_cleanup;
use operon_dioxus::plugins::skill::materialize;

struct Harness {
    _vault_tmp: tempfile::TempDir,
    _repo_tmp: tempfile::TempDir,
    repo_path: PathBuf,
    vault: VaultRoot,
    note_repo: Arc<dyn LocalNoteRepository>,
    project_repo: Arc<dyn LocalProjectRepository>,
    persistence: Arc<dyn Persistence>,
    project_id: uuid::Uuid,
}

fn make_harness() -> Harness {
    let store = Store::open_in_memory().expect("sqlite in-memory");
    let project_repo: Arc<dyn LocalProjectRepository> =
        Arc::new(SqliteLocalProjectRepository::new(store.clone()));
    let raw_notes: Arc<dyn LocalNoteRepository> =
        Arc::new(SqliteLocalNoteRepository::new(store.clone()));
    // Same wrapping the app uses so artifact dirs relocate on rename and
    // get pre-cleaned by the wrapper on delete. The cleanup helper does
    // its own remove_dir_all so the wrapper is not strictly required —
    // but wrapping matches production wiring.
    let vault_tmp = tempfile::tempdir().unwrap();
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo_path = repo_tmp.path().to_path_buf();

    let project = project_repo.create("alpha").unwrap();
    project_repo
        .set_repo_path(project.id, Some(repo_path.as_path()))
        .unwrap();

    let notes_dir = vault_tmp.path().join("notes");
    let persistence: Arc<dyn Persistence> =
        Arc::new(FilesystemPersistence::new(&notes_dir).unwrap());

    let vault = VaultRoot {
        path: vault_tmp.path().to_path_buf(),
    };

    let note_repo: Arc<dyn LocalNoteRepository> = Arc::new(RelocatingNoteRepo::new(
        raw_notes,
        Some(vault.clone()),
    ));

    Harness {
        _vault_tmp: vault_tmp,
        _repo_tmp: repo_tmp,
        repo_path,
        vault,
        note_repo,
        project_repo,
        persistence,
        project_id: project.id,
    }
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn deleting_skill_note_removes_materialized_file() {
    let h = make_harness();

    let skill = h
        .note_repo
        .create_with_kind(h.project_id, None, "Decompose Epic", NoteKind::Skill)
        .unwrap();

    // Materialize the skill the same way the Play button does.
    let slug = "ba-decompose-epic";
    let body = "---\nskill_name: ba-decompose-epic\n---\n\nyou are a BA";
    block(
        h.persistence
            .save(&skill.id.to_string(), body.as_bytes()),
    )
    .unwrap();
    materialize::write_skill_to_repo(&h.repo_path, slug, body).unwrap();

    let materialized = h.repo_path.join(".claude").join("skills").join(format!("{slug}.md"));
    assert!(materialized.is_file(), "precondition: skill is materialized");

    let outcome = block(delete_note_with_disk_cleanup(
        skill.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();

    assert!(
        !materialized.exists(),
        "skill .md should be moved out of original location"
    );
    assert!(!outcome.trash.is_empty(), "trash record captured the move");
}

#[test]
fn undo_after_skill_delete_restores_materialized_file() {
    let h = make_harness();

    let skill = h
        .note_repo
        .create_with_kind(h.project_id, None, "Compose", NoteKind::Skill)
        .unwrap();
    let slug = "ba-compose";
    let body = "---\nskill_name: ba-compose\n---\n\nbody";
    block(h.persistence.save(&skill.id.to_string(), body.as_bytes())).unwrap();
    materialize::write_skill_to_repo(&h.repo_path, slug, body).unwrap();
    let materialized = h.repo_path.join(".claude").join("skills").join(format!("{slug}.md"));

    let outcome = block(delete_note_with_disk_cleanup(
        skill.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();
    assert!(!materialized.exists());

    // Simulate Ctrl+Z: restore SQLite + restore trashed files.
    h.note_repo.restore_subtree(&outcome.snapshot.unwrap()).unwrap();
    outcome.trash.restore();

    assert!(materialized.is_file(), "skill restored to original location");
    // Body should match (the materialized form, post-compat wrap).
    let restored = std::fs::read_to_string(&materialized).unwrap();
    assert!(restored.contains("body"));
}

#[test]
fn deleting_skill_without_declared_name_uses_note_id_slug() {
    let h = make_harness();

    let skill = h
        .note_repo
        .create_with_kind(h.project_id, None, "Anon", NoteKind::Skill)
        .unwrap();

    // No `skill_name:` in frontmatter — materialize uses the slugified note id.
    let body = "no frontmatter here, just prose";
    block(
        h.persistence
            .save(&skill.id.to_string(), body.as_bytes()),
    )
    .unwrap();
    let id_slug = operon_dioxus::plugins::skill::frontmatter::slugify(&skill.id.to_string());
    materialize::write_skill_to_repo(&h.repo_path, &id_slug, body).unwrap();

    let materialized = h.repo_path.join(".claude").join("skills").join(format!("{id_slug}.md"));
    assert!(materialized.is_file());

    let _ = block(delete_note_with_disk_cleanup(
        skill.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();

    assert!(!materialized.exists());
}

#[test]
fn deleting_parent_cleans_descendant_skills() {
    let h = make_harness();

    let parent = h
        .note_repo
        .create(h.project_id, None, "container")
        .unwrap();
    let skill_a = h
        .note_repo
        .create_with_kind(h.project_id, Some(parent.id), "A", NoteKind::Skill)
        .unwrap();
    let skill_b = h
        .note_repo
        .create_with_kind(h.project_id, Some(parent.id), "B", NoteKind::Skill)
        .unwrap();

    let body_a = "---\nskill_name: alpha\n---\n\nA";
    let body_b = "---\nskill_name: beta\n---\n\nB";
    block(h.persistence.save(&skill_a.id.to_string(), body_a.as_bytes())).unwrap();
    block(h.persistence.save(&skill_b.id.to_string(), body_b.as_bytes())).unwrap();
    materialize::write_skill_to_repo(&h.repo_path, "alpha", body_a).unwrap();
    materialize::write_skill_to_repo(&h.repo_path, "beta", body_b).unwrap();

    let alpha = h.repo_path.join(".claude").join("skills").join("alpha.md");
    let beta = h.repo_path.join(".claude").join("skills").join("beta.md");
    assert!(alpha.exists() && beta.exists());

    let _ = block(delete_note_with_disk_cleanup(
        parent.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();

    assert!(!alpha.exists(), "descendant skill A should be cleaned up");
    assert!(!beta.exists(), "descendant skill B should be cleaned up");
}

#[test]
fn deleting_artifact_removes_on_disk_dir() {
    let h = make_harness();

    let artifact = h
        .note_repo
        .create_with_kind(h.project_id, None, "Login Epic", NoteKind::Artifact)
        .unwrap();

    // The wrapper assigns a slug at create time for artifacts.
    let slug = h
        .note_repo
        .list_for_project(h.project_id)
        .unwrap()
        .into_iter()
        .find(|n| n.id == artifact.id)
        .and_then(|n| n.slug)
        .expect("artifact has a slug after create");

    let dir = h
        .vault
        .project_artifacts_dir(h.project_id)
        .join(&slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.md"), "body").unwrap();
    assert!(dir.is_dir());

    let outcome = block(delete_note_with_disk_cleanup(
        artifact.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();

    assert!(!dir.exists(), "artifact dir should be moved out of original location");
    assert!(!outcome.trash.is_empty(), "trash captured the artifact dir");

    // Undo round-trip: restore SQLite + trash.
    h.note_repo.restore_subtree(&outcome.snapshot.unwrap()).unwrap();
    outcome.trash.restore();
    assert!(dir.is_dir(), "artifact dir restored at original location");
    assert!(dir.join("index.md").is_file());
}

#[test]
fn deleting_image_note_gcs_unreferenced_blob_only() {
    let h = make_harness();

    // Create two image notes referencing the same blob.
    let img_a = h
        .note_repo
        .create_with_kind(h.project_id, None, "img-a", NoteKind::Image)
        .unwrap();
    let img_b = h
        .note_repo
        .create_with_kind(h.project_id, None, "img-b", NoteKind::Image)
        .unwrap();
    let shared_blob = "blobs/shared.png";
    h.note_repo
        .set_blob_path(img_a.id, Some(shared_blob))
        .unwrap();
    h.note_repo
        .set_blob_path(img_b.id, Some(shared_blob))
        .unwrap();

    // Lay down the blob on disk.
    let blob_dir = h.vault.path().join("blobs");
    std::fs::create_dir_all(&blob_dir).unwrap();
    let blob_path = h.vault.path().join(shared_blob);
    std::fs::write(&blob_path, b"pngdata").unwrap();
    assert!(blob_path.exists());

    // Delete img-a — img-b still references the blob; file should remain.
    let outcome_a = block(delete_note_with_disk_cleanup(
        img_a.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();
    assert!(
        blob_path.exists(),
        "blob still referenced by img-b — must not be moved"
    );
    assert!(
        outcome_a.trash.is_empty(),
        "no trash moves while another note references the blob"
    );

    // Delete img-b — now nothing references the blob; it should be trashed.
    let outcome_b = block(delete_note_with_disk_cleanup(
        img_b.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();
    assert!(
        !blob_path.exists(),
        "blob is now orphaned — must be moved out of original location"
    );
    assert!(!outcome_b.trash.is_empty(), "blob move recorded");

    // Undo of the second delete restores the blob.
    h.note_repo
        .restore_subtree(&outcome_b.snapshot.unwrap())
        .unwrap();
    outcome_b.trash.restore();
    assert!(blob_path.is_file(), "blob restored at original path");
}

#[test]
fn purge_actually_removes_trashed_files_from_disk() {
    let h = make_harness();
    let skill = h
        .note_repo
        .create_with_kind(h.project_id, None, "Doomed", NoteKind::Skill)
        .unwrap();
    let body = "---\nskill_name: doomed\n---\n\nbody";
    block(h.persistence.save(&skill.id.to_string(), body.as_bytes())).unwrap();
    materialize::write_skill_to_repo(&h.repo_path, "doomed", body).unwrap();

    let outcome = block(delete_note_with_disk_cleanup(
        skill.id,
        &h.note_repo,
        &h.project_repo,
        &h.persistence,
        Some(&h.vault),
    ))
    .unwrap();

    // Trash dir for this delete exists. Skill files live under the
    // repo's `.claude/`, so their trash root is co-located there.
    let skill_trash_root = h.repo_path.join(".claude").join("trash");
    let trash_dir = skill_trash_root.join(outcome.trash.trash_id.to_string());
    assert!(trash_dir.is_dir());

    outcome.trash.purge(
        &skill_trash_root,
        Some(&h.vault.path().join(".operon").join("trash")),
    );
    assert!(!trash_dir.exists(), "purge removes the per-delete trash dir");
}
