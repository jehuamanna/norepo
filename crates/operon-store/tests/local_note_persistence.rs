//! Migration #007 + LocalNote / LocalTreeState repo persistence integration test.

use operon_store::repos::{
    LocalNoteRepository, LocalProjectRepository, LocalTreeStateRepository,
    SqliteLocalNoteRepository, SqliteLocalProjectRepository, SqliteLocalTreeStateRepository,
};
use operon_store::{Store, StoreConfig};

#[test]
fn local_note_persists_across_reopen() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let _ = tmp.as_file();

    let (project_id, root_id, child_id) = {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let projects = SqliteLocalProjectRepository::new(store.clone());
        let notes = SqliteLocalNoteRepository::new(store);
        let project = projects.create("alpha").unwrap();
        let root = notes.create(project.id, None, "root").unwrap();
        let child = notes.create(project.id, Some(root.id), "child").unwrap();
        (project.id, root.id, child.id)
    };

    {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let notes = SqliteLocalNoteRepository::new(store);
        let listed = notes.list_for_project(project_id).unwrap();
        assert_eq!(listed.len(), 2);
        let root = listed.iter().find(|x| x.id == root_id).unwrap();
        let child = listed.iter().find(|x| x.id == child_id).unwrap();
        assert_eq!(root.title, "root");
        assert_eq!(root.depth, 0);
        assert!(root.parent_id.is_none());
        assert_eq!(child.title, "child");
        assert_eq!(child.depth, 1);
        assert_eq!(child.parent_id, Some(root_id));
    }
}

#[test]
fn local_tree_state_persists_across_reopen() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let _ = tmp.as_file();

    {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let repo = SqliteLocalTreeStateRepository::new(store);
        repo.set("workspace", "project-a", true).unwrap();
        repo.set("workspace", "project-b", false).unwrap();
        repo.set("project:1", "note-x", true).unwrap();
    }

    {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let repo = SqliteLocalTreeStateRepository::new(store);
        assert!(repo.is_open("workspace", "project-a").unwrap());
        assert!(!repo.is_open("workspace", "project-b").unwrap());
        assert!(repo.is_open("project:1", "note-x").unwrap());

        let snap = repo.snapshot_for_scope("workspace").unwrap();
        assert_eq!(snap.get("project-a"), Some(&true));
        assert_eq!(snap.get("project-b"), Some(&false));
    }
}

#[test]
fn local_note_cascade_deletes_when_project_deleted() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let _ = tmp.as_file();

    let store = Store::open(StoreConfig::local(&path)).unwrap();
    let projects = SqliteLocalProjectRepository::new(store.clone());
    let notes = SqliteLocalNoteRepository::new(store);
    let project = projects.create("alpha").unwrap();
    let root = notes.create(project.id, None, "root").unwrap();
    let _child = notes.create(project.id, Some(root.id), "child").unwrap();

    projects.delete(project.id).unwrap();
    let listed = notes.list_for_project(project.id).unwrap();
    assert!(listed.is_empty());
}
