//! Shared note-title lookup used by the companion chat (mention chips,
//! drag-drop, right-click) and the `NoteTitleResolver` desktop context
//! that drives live chip-title reactivity on rename.
//!
//! Kept in its own module so callers in `src/shell` and `src/local_mode`
//! don't have to depend on each other.

use std::collections::HashMap;
use std::sync::Arc;

use operon_store::repos::{LocalNoteRepository, LocalProjectRepository};
use uuid::Uuid;

/// Resolve a note UUID to its display title. Tries
/// `find_project_for_note` first (O(1) on the SQLite repo); falls back
/// to scanning every project's note list (works on repos that don't
/// override the default `find_project_for_note`). Returns `None` when
/// the note isn't found in any project — callers fall back to
/// displaying the frozen title embedded in the mention token or the
/// bare UUID.
pub fn lookup_note_title(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: Option<&Arc<dyn LocalProjectRepository>>,
    note_id: Uuid,
) -> Option<String> {
    if let Ok(Some(pid)) = note_repo.find_project_for_note(note_id) {
        if let Ok(notes) = note_repo.list_for_project(pid) {
            if let Some(n) = notes.into_iter().find(|n| n.id == note_id) {
                return Some(n.title);
            }
        }
    }
    let project_repo = project_repo?;
    let projects = project_repo.list().ok()?;
    for p in projects {
        if let Ok(notes) = note_repo.list_for_project(p.id) {
            if let Some(n) = notes.into_iter().find(|n| n.id == note_id) {
                return Some(n.title);
            }
        }
    }
    None
}

/// Resolve a note UUID to a hierarchical display path
/// `"<project> / <parent> / … / <leaf>"`. Walks the in-memory note
/// list once via `parent_id` to assemble the chain, so it's one DB
/// call per note plus one project lookup.
///
/// Returns `None` when the note's project can't be located or the
/// project repo isn't available — callers fall back to the bare
/// title.
pub fn lookup_note_path(
    note_repo: &Arc<dyn LocalNoteRepository>,
    project_repo: Option<&Arc<dyn LocalProjectRepository>>,
    note_id: Uuid,
) -> Option<String> {
    let project_id = note_repo.find_project_for_note(note_id).ok().flatten()?;
    let project_repo = project_repo?;
    let project_name = project_repo
        .list()
        .ok()?
        .into_iter()
        .find(|p| p.id == project_id)
        .map(|p| p.name)?;
    let notes = note_repo.list_for_project(project_id).ok()?;
    let by_id: HashMap<Uuid, (Option<Uuid>, String)> = notes
        .into_iter()
        .map(|n| (n.id, (n.parent_id, n.title)))
        .collect();
    let leaf = by_id.get(&note_id)?;
    let mut chain: Vec<String> = vec![leaf.1.clone()];
    let mut current = leaf.0;
    while let Some(pid) = current {
        if let Some((parent_of_parent, title)) = by_id.get(&pid) {
            chain.push(title.clone());
            current = *parent_of_parent;
        } else {
            break;
        }
    }
    chain.reverse();
    let mut s = String::with_capacity(project_name.len() + 6);
    s.push_str(&project_name);
    for t in chain {
        s.push_str(" / ");
        s.push_str(&t);
    }
    Some(s)
}
