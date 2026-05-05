//! Integration tests for [`LocalSearchRepository`]. Cross-project sanity:
//! same-named notes in two projects must surface separately, each with the
//! correct breadcrumb prefix.

use operon_store::repos::{
    LocalNoteRepository, LocalProjectRepository, LocalSearchRepository, SearchKind,
    SqliteLocalNoteRepository, SqliteLocalProjectRepository, SqliteLocalSearchRepository,
    DEFAULT_SEARCH_LIMIT,
};
use operon_store::test_support::open_in_memory;
use uuid::Uuid;

#[test]
fn local_search_does_not_leak_across_projects_unexpectedly() {
    let store = open_in_memory().unwrap();
    let projects = SqliteLocalProjectRepository::new(store.clone());
    let notes = SqliteLocalNoteRepository::new(store.clone());
    let search = SqliteLocalSearchRepository::new(store);

    let p1 = projects.create("ProjectOne").unwrap();
    let p2 = projects.create("ProjectTwo").unwrap();

    // Same title in both projects.
    let n1 = notes.create(p1.id, None, "Shared Note").unwrap();
    let n2 = notes.create(p2.id, None, "Shared Note").unwrap();

    let loader: Box<dyn Fn(Uuid) -> Option<String>> = Box::new(|_| None);
    let hits = search
        .search("Shared Note", false, DEFAULT_SEARCH_LIMIT, &*loader)
        .unwrap();

    let note_hits: Vec<_> = hits.iter().filter(|h| h.kind == SearchKind::Note).collect();
    assert_eq!(note_hits.len(), 2, "expected one hit per project");

    let crumbs: std::collections::HashSet<String> =
        note_hits.iter().map(|h| h.breadcrumb.clone()).collect();
    assert!(crumbs.contains("ProjectOne / Shared Note"));
    assert!(crumbs.contains("ProjectTwo / Shared Note"));

    // Each hit must point at its own project_id.
    let hit_for_n1 = note_hits.iter().find(|h| h.id == n1.id).unwrap();
    let hit_for_n2 = note_hits.iter().find(|h| h.id == n2.id).unwrap();
    assert_eq!(hit_for_n1.project_id, Some(p1.id));
    assert_eq!(hit_for_n2.project_id, Some(p2.id));
}
