//! Group + walk a flat `Vec<LocalNote>` (sorted by parent_id, sibling_index)
//! into a renderable tree, respecting per-node open/closed state.

use std::collections::HashMap;

use operon_store::repos::LocalNote;
use uuid::Uuid;

/// Children-by-parent index, plus a "roots" list for `parent_id IS NULL`. Used
/// by the explorer panel to walk the tree depth-first when rendering rows.
pub struct NoteForest {
    pub roots: Vec<LocalNote>,
    pub children: HashMap<Uuid, Vec<LocalNote>>,
}

impl NoteForest {
    /// Build from a flat list. Input is expected to be `LocalNoteRepository::list_for_project`'s
    /// output (already sorted by parent / sibling_index). The function does not
    /// re-sort — callers depend on stable ordering.
    pub fn from_flat(notes: Vec<LocalNote>) -> Self {
        let mut roots = Vec::new();
        let mut children: HashMap<Uuid, Vec<LocalNote>> = HashMap::new();
        for n in notes {
            match n.parent_id {
                Some(pid) => children.entry(pid).or_default().push(n),
                None => roots.push(n),
            }
        }
        Self { roots, children }
    }

    pub fn children_of(&self, id: &Uuid) -> &[LocalNote] {
        self.children.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn has_children(&self, id: &Uuid) -> bool {
        self.children
            .get(id)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }
}

/// Walk the forest depth-first, yielding `(note, is_open_for_subtree)` for
/// every note that should currently render. A node is rendered when all of
/// its ancestors are open. Called by the explorer panel which then maps each
/// yielded note to a [`super::NoteRow`].
pub fn flatten_visible(forest: &NoteForest, is_open: &dyn Fn(&Uuid) -> bool) -> Vec<LocalNote> {
    let mut out = Vec::new();
    for root in forest.roots.iter() {
        push_subtree(forest, root, is_open, &mut out);
    }
    out
}

fn push_subtree(
    forest: &NoteForest,
    note: &LocalNote,
    is_open: &dyn Fn(&Uuid) -> bool,
    out: &mut Vec<LocalNote>,
) {
    out.push(note.clone());
    if is_open(&note.id) {
        for child in forest.children_of(&note.id) {
            push_subtree(forest, child, is_open, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn note(id: Uuid, parent: Option<Uuid>, depth: i64, sibling: i64, title: &str) -> LocalNote {
        LocalNote {
            id,
            project_id: Uuid::nil(),
            parent_id: parent,
            sibling_index: sibling,
            depth,
            title: title.into(),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn forest_groups_roots_and_children() {
        let r1 = Uuid::new_v4();
        let r2 = Uuid::new_v4();
        let c1 = Uuid::new_v4();
        let c2 = Uuid::new_v4();
        let flat = vec![
            note(r1, None, 0, 0, "r1"),
            note(r2, None, 0, 1, "r2"),
            note(c1, Some(r1), 1, 0, "c1"),
            note(c2, Some(r1), 1, 1, "c2"),
        ];
        let forest = NoteForest::from_flat(flat);
        assert_eq!(forest.roots.len(), 2);
        assert_eq!(forest.children_of(&r1).len(), 2);
        assert!(forest.children_of(&r2).is_empty());
        assert!(forest.has_children(&r1));
        assert!(!forest.has_children(&r2));
    }

    #[test]
    fn flatten_visible_skips_closed_subtrees() {
        let r1 = Uuid::new_v4();
        let c1 = Uuid::new_v4();
        let g1 = Uuid::new_v4();
        let flat = vec![
            note(r1, None, 0, 0, "r1"),
            note(c1, Some(r1), 1, 0, "c1"),
            note(g1, Some(c1), 2, 0, "g1"),
        ];
        let forest = NoteForest::from_flat(flat);

        let closed: HashSet<Uuid> = HashSet::new();
        let visible = flatten_visible(&forest, &|id| closed.contains(id));
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, r1);

        let mut open: HashSet<Uuid> = HashSet::new();
        open.insert(r1);
        let visible = flatten_visible(&forest, &|id| open.contains(id));
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[1].id, c1);

        open.insert(c1);
        let visible = flatten_visible(&forest, &|id| open.contains(id));
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[2].id, g1);
    }
}
