//! Auto-managed Contents section ("TOC") for any note.
//!
//! A note opts in by having `<!-- operon:toc -->` anywhere in its body —
//! everything from that sentinel to EOF is treated as auto-generated
//! and is regenerated on each load. Content above the sentinel is the
//! user's preamble and is preserved verbatim.
//!
//! CE / Phase / Artifact notes get the sentinel seeded automatically at
//! creation (see `seed_sentinel`). Other notes can opt in via the
//! explorer's "Insert Contents" action — same machinery, just a
//! user-initiated trigger.
//!
//! The companion piece — running build + render + splice on note load —
//! lives wherever notes are loaded for display; see the lifecycle hook
//! in the Shell.
//!
//! Same "preamble + trailing managed section" shape as the SKILLS
//! index (`local_mode/explorer/project_row.rs`); we just key on a
//! sentinel comment instead of a hardcoded heading so users can
//! author their own preamble structure freely.
//!
//! ## Identity of the managed section
//!
//! The full managed block is:
//!
//! ```text
//! <!-- operon:toc -->
//! ## Contents
//!
//! - [title](operon://note/<uuid>)
//!   - [child](operon://note/<uuid>)
//! ```
//!
//! Removing the sentinel line opts a note back out — there's no
//! separate flag in SQL, presence of the comment is the state.
//!
//! ## What appears in the TOC
//!
//! Direct + transitive children of the rooted note (depth-first), in
//! `(kind_sort_key, created_at_ms)` order so the listing is stable
//! across reloads. Image notes are filtered via
//! `NoteKind::shows_in_toc()` since they don't render as plain links.

use operon_store::repos::{LocalNote, NoteKind};
use std::collections::HashMap;
use uuid::Uuid;

pub const TOC_SENTINEL: &str = "<!-- operon:toc -->";
pub const TOC_HEADING: &str = "## Contents";
pub const EMPTY_PLACEHOLDER: &str = "_(no children yet)_";

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub note_id: Uuid,
    pub title: String,
    pub children: Vec<TocEntry>,
}

/// Build the recursive entry tree rooted at `root_id` from a flat
/// `list_for_project` result. Children at each level are sorted by
/// `(kind_sort_key, created_at_ms)` so reloads are stable. Notes whose
/// `kind.shows_in_toc()` is false are skipped (and their subtrees with
/// them — Images don't have meaningful children either).
pub fn build_toc(root_id: Uuid, notes: &[LocalNote]) -> Vec<TocEntry> {
    let mut by_parent: HashMap<Uuid, Vec<&LocalNote>> = HashMap::new();
    for n in notes {
        if let Some(pid) = n.parent_id {
            by_parent.entry(pid).or_default().push(n);
        }
    }
    for v in by_parent.values_mut() {
        v.sort_by_key(|n| (kind_sort_key(n.kind), n.created_at_ms, n.sibling_index));
    }
    descend(root_id, &by_parent)
}

fn descend(parent: Uuid, by_parent: &HashMap<Uuid, Vec<&LocalNote>>) -> Vec<TocEntry> {
    let Some(kids) = by_parent.get(&parent) else {
        return Vec::new();
    };
    kids.iter()
        .filter(|n| n.kind.shows_in_toc())
        .map(|n| TocEntry {
            note_id: n.id,
            title: n.title.clone(),
            children: descend(n.id, by_parent),
        })
        .collect()
}

/// Render the full managed block: sentinel + `## Contents` heading +
/// indented bullet list (or the empty placeholder if `entries` is
/// empty). Two-space indent per depth level; entries link via the
/// `operon://note/<uuid>` scheme the editor already recognises.
pub fn render_toc(entries: &[TocEntry]) -> String {
    let mut out = String::new();
    out.push_str(TOC_SENTINEL);
    out.push('\n');
    out.push_str(TOC_HEADING);
    out.push_str("\n\n");
    if entries.is_empty() {
        out.push_str(EMPTY_PLACEHOLDER);
        out.push('\n');
        return out;
    }
    for e in entries {
        write_entry(&mut out, e, 0);
    }
    out
}

fn write_entry(out: &mut String, e: &TocEntry, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str("- [");
    out.push_str(escape_link_title(&e.title).as_ref());
    out.push_str("](operon://note/");
    out.push_str(&e.note_id.to_string());
    out.push_str(")\n");
    for c in &e.children {
        write_entry(out, c, depth + 1);
    }
}

fn escape_link_title(title: &str) -> std::borrow::Cow<'_, str> {
    if title.contains(['[', ']']) {
        std::borrow::Cow::Owned(title.replace('[', "\\[").replace(']', "\\]"))
    } else {
        std::borrow::Cow::Borrowed(title)
    }
}

/// Splice `rendered_toc` into `body`. If the sentinel is present, the
/// preamble (everything before the sentinel) is preserved and the
/// managed block is replaced. If absent, the rendered block is
/// appended with one blank line separating it from the existing body.
pub fn splice_toc(body: &str, rendered_toc: &str) -> String {
    if let Some(idx) = body.find(TOC_SENTINEL) {
        let mut out = String::with_capacity(idx + rendered_toc.len());
        out.push_str(&body[..idx]);
        out.push_str(rendered_toc);
        out
    } else {
        let trimmed = body.trim_end_matches('\n');
        if trimmed.is_empty() {
            rendered_toc.to_string()
        } else {
            format!("{trimmed}\n\n{rendered_toc}")
        }
    }
}

/// Seed an empty managed block onto a body that has none. Used at
/// creation time for CE / Phase / Artifact notes so the auto-managed
/// boundary is visible from minute zero. Idempotent: re-invoking on a
/// body that already contains the sentinel is a no-op.
pub fn seed_sentinel(body: &str) -> String {
    if body.contains(TOC_SENTINEL) {
        return body.to_string();
    }
    splice_toc(body, &render_toc(&[]))
}

/// Refresh the managed Contents section on note load. Returns the body
/// unchanged if the sentinel is absent (note isn't opted in), otherwise
/// returns a body with the Contents block regenerated against the
/// current subtree. Pure function — the caller is responsible for
/// persisting the result if it differs from the input.
pub fn refresh_if_managed(body: &str, note_id: Uuid, notes: &[LocalNote]) -> String {
    if !body.contains(TOC_SENTINEL) {
        return body.to_string();
    }
    let entries = build_toc(note_id, notes);
    let rendered = render_toc(&entries);
    splice_toc(body, &rendered)
}

fn kind_sort_key(k: NoteKind) -> u8 {
    // Group structural / hierarchical kinds first, then content kinds.
    // Within each group, the relative order is what shows up in TOC
    // listings — the goal is "scaffolding kinds first, free-form
    // content last".
    match k {
        NoteKind::Phase => 0,
        NoteKind::Ce => 1,
        NoteKind::Artifact => 2,
        NoteKind::Workflow => 3,
        NoteKind::Skill => 4,
        NoteKind::Markdown => 5,
        NoteKind::Mdx => 6,
        NoteKind::Code => 7,
        NoteKind::Canvas => 8,
        NoteKind::Excalidraw => 9,
        NoteKind::Kanban => 10,
        NoteKind::Image => 11,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(
        id: Uuid,
        parent: Option<Uuid>,
        title: &str,
        kind: NoteKind,
        created_at_ms: i64,
    ) -> LocalNote {
        LocalNote {
            id,
            project_id: Uuid::nil(),
            parent_id: parent,
            sibling_index: 0,
            depth: 0,
            title: title.into(),
            created_at_ms,
            updated_at_ms: created_at_ms,
            kind,
            blob_path: None,
            slug: None,
        }
    }

    #[test]
    fn build_toc_empty_subtree() {
        let root = Uuid::new_v4();
        let notes = vec![note(root, None, "Root", NoteKind::Phase, 0)];
        let entries = build_toc(root, &notes);
        assert!(entries.is_empty());
    }

    #[test]
    fn build_toc_recursive_descent() {
        let root = Uuid::new_v4();
        let c1 = Uuid::new_v4();
        let c2 = Uuid::new_v4();
        let g1 = Uuid::new_v4();
        let notes = vec![
            note(root, None, "Root", NoteKind::Phase, 0),
            note(c1, Some(root), "Child 1", NoteKind::Artifact, 1),
            note(c2, Some(root), "Child 2", NoteKind::Artifact, 2),
            note(g1, Some(c1), "Grandchild", NoteKind::Artifact, 3),
        ];
        let entries = build_toc(root, &notes);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Child 1");
        assert_eq!(entries[0].children.len(), 1);
        assert_eq!(entries[0].children[0].title, "Grandchild");
        assert_eq!(entries[1].title, "Child 2");
        assert!(entries[1].children.is_empty());
    }

    #[test]
    fn build_toc_skips_images() {
        let root = Uuid::new_v4();
        let img = Uuid::new_v4();
        let md = Uuid::new_v4();
        let notes = vec![
            note(root, None, "Root", NoteKind::Phase, 0),
            note(img, Some(root), "Sketch", NoteKind::Image, 1),
            note(md, Some(root), "Notes", NoteKind::Markdown, 2),
        ];
        let entries = build_toc(root, &notes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Notes");
    }

    #[test]
    fn render_toc_empty_emits_placeholder() {
        let out = render_toc(&[]);
        assert!(out.starts_with(TOC_SENTINEL));
        assert!(out.contains(TOC_HEADING));
        assert!(out.contains(EMPTY_PLACEHOLDER));
    }

    #[test]
    fn render_toc_nested_indent() {
        let entries = vec![TocEntry {
            note_id: Uuid::nil(),
            title: "Parent".into(),
            children: vec![TocEntry {
                note_id: Uuid::nil(),
                title: "Kid".into(),
                children: vec![],
            }],
        }];
        let out = render_toc(&entries);
        assert!(out.contains("- [Parent]"));
        assert!(out.contains("  - [Kid]"));
        assert!(out.contains("operon://note/"));
    }

    #[test]
    fn render_toc_escapes_brackets_in_title() {
        let entries = vec![TocEntry {
            note_id: Uuid::nil(),
            title: "Edge [case]".into(),
            children: vec![],
        }];
        let out = render_toc(&entries);
        assert!(out.contains("Edge \\[case\\]"));
    }

    #[test]
    fn splice_toc_appends_when_no_sentinel() {
        let body = "# Title\n\nUser prose.\n";
        let rendered = render_toc(&[]);
        let out = splice_toc(body, &rendered);
        assert!(out.starts_with("# Title\n\nUser prose."));
        assert!(out.contains(TOC_SENTINEL));
        // Exactly one blank line between preamble and managed block.
        assert!(out.contains("User prose.\n\n<!-- operon:toc -->"));
    }

    #[test]
    fn splice_toc_replaces_when_sentinel_present() {
        let body = format!("# Title\n\nKeep me.\n\n{}\n## Contents\n\n- stale\n", TOC_SENTINEL);
        let entries = vec![TocEntry {
            note_id: Uuid::nil(),
            title: "Fresh".into(),
            children: vec![],
        }];
        let out = splice_toc(&body, &render_toc(&entries));
        assert!(out.contains("Keep me."));
        assert!(!out.contains("- stale"));
        assert!(out.contains("- [Fresh]"));
        // The original preamble must not be duplicated.
        assert_eq!(out.matches("# Title").count(), 1);
    }

    #[test]
    fn splice_toc_handles_empty_body() {
        let out = splice_toc("", &render_toc(&[]));
        assert!(out.starts_with(TOC_SENTINEL));
    }

    #[test]
    fn seed_sentinel_idempotent() {
        let body = "# Title\n\nProse\n";
        let once = seed_sentinel(body);
        let twice = seed_sentinel(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn seed_sentinel_appends_empty_block() {
        let out = seed_sentinel("# Title\n\nProse\n");
        assert!(out.contains(TOC_SENTINEL));
        assert!(out.contains(EMPTY_PLACEHOLDER));
    }

    #[test]
    fn deep_recursion_does_not_panic() {
        // Build a deep chain a -> b -> c -> ... 50 deep to confirm
        // descent doesn't trip on linear chains.
        let mut notes = Vec::new();
        let root = Uuid::new_v4();
        notes.push(note(root, None, "0", NoteKind::Phase, 0));
        let mut prev = root;
        for i in 1..50 {
            let id = Uuid::new_v4();
            notes.push(note(id, Some(prev), &i.to_string(), NoteKind::Artifact, i));
            prev = id;
        }
        let entries = build_toc(root, &notes);
        // Walk down and confirm the depth.
        let mut depth = 0;
        let mut cur = entries.as_slice();
        while let Some(e) = cur.first() {
            depth += 1;
            cur = e.children.as_slice();
        }
        assert_eq!(depth, 49);
    }
}
