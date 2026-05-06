//! Cross-aggregate search across `local_project` (project name) and `local_note`
//! (title; optionally body via a caller-provided closure). All comparisons are
//! parameterised — no string concatenation of user input into SQL.

use crate::sql::params;
use uuid::Uuid;

use crate::error::StoreError;
use crate::repos::local_note::NoteKind;
use crate::store::Store;

/// Cap a single `search()` call to this many hits unless the caller passes a
/// smaller `limit`. The UI shows `+ N more` if the cap is hit.
pub const DEFAULT_SEARCH_LIMIT: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchKind {
    Project,
    Note,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub kind: SearchKind,
    pub id: Uuid,
    /// `None` for project hits, `Some(project_id)` for note hits so the caller
    /// can expand the right project on click.
    pub project_id: Option<Uuid>,
    pub title: String,
    /// `"Project name"` for project hits, `"Project name / Note title"` for notes.
    pub breadcrumb: String,
    /// Populated only when the match came from the note body.
    pub snippet: Option<String>,
    /// `None` for project hits, `Some(NoteKind)` for note hits. Lets the
    /// link-picker render a `[md]` / `[im]` badge per row and choose
    /// between `[[…]]` and `![[…]]` syntax when inserting the reference.
    pub note_kind: Option<NoteKind>,
}

pub trait LocalSearchRepository: Send + Sync {
    /// Search both projects (by name) and notes (by title; plus body when
    /// `in_content == true`). The closure is only invoked for body scanning;
    /// when `in_content == false` it is never called.
    fn search(
        &self,
        query: &str,
        in_content: bool,
        limit: usize,
        body_loader: &dyn Fn(Uuid) -> Option<String>,
    ) -> Result<Vec<SearchHit>, StoreError>;
}

pub struct SqliteLocalSearchRepository {
    store: Store,
}

impl SqliteLocalSearchRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid_uuid(s: String) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid uuid: {s}"),
        )),
    )
}

/// Plans-Phase-9-wikilink-picker (rev 2): walk the parent chain for a
/// note id so the link-picker breadcrumb can show the full path
/// (`Project / Parent / Sub / Title`) instead of just `Project / Title`.
/// Returns parent titles in root-to-leaf order, *excluding* the leaf
/// itself. A 64-step depth cap defends against cyclic parent pointers
/// (move-to rejects cycles, but don't loop the search if the DB ever
/// holds bad data).
fn parent_chain_titles(conn: &crate::sql::Connection, leaf_id: Uuid) -> Vec<String> {
    let sql = "WITH RECURSIVE chain(id, parent_id, title, depth) AS (
            SELECT id, parent_id, title, 0 FROM local_note WHERE id = ?1
            UNION ALL
            SELECT n.id, n.parent_id, n.title, c.depth + 1
            FROM local_note n JOIN chain c ON n.id = c.parent_id
            WHERE c.depth < 64
        )
        SELECT title FROM chain WHERE depth > 0 ORDER BY depth DESC";
    let mut titles: Vec<String> = Vec::new();
    let Ok(mut stmt) = conn.prepare(sql) else {
        return titles;
    };
    let rows = stmt.query_map(params![leaf_id.to_string()], |row| row.get::<_, String>(0));
    if let Ok(iter) = rows {
        for r in iter.flatten() {
            titles.push(r);
        }
    }
    titles
}

/// Compose a breadcrumb from the project name, walked parent titles,
/// and the leaf title — `Project / Parent / Sub / Title` (or
/// `Project / Title` for top-level notes).
fn compose_breadcrumb(project_name: &str, parent_titles: &[String], leaf_title: &str) -> String {
    let cap = project_name.len()
        + leaf_title.len()
        + 6
        + parent_titles.iter().map(|s| s.len() + 3).sum::<usize>();
    let mut s = String::with_capacity(cap);
    s.push_str(project_name);
    for t in parent_titles {
        s.push_str(" / ");
        s.push_str(t);
    }
    s.push_str(" / ");
    s.push_str(leaf_title);
    s
}

/// Build a snippet around the first match. Takes up to 60 chars before and 60
/// chars after the match offset, then trims/normalises run of whitespace and
/// newlines into single spaces.
fn build_snippet(body: &str, match_byte_offset: usize) -> String {
    // Use char-aware windows to avoid slicing through multi-byte UTF-8.
    let lower = body.to_lowercase();
    // Map byte offset back to char offset in the lowercase view (same lengths
    // for ASCII-fast-path searches; `to_lowercase` may alter length for some
    // codepoints, but the snippet is still safe because we cut by char index).
    let _ = lower;
    // Recompute by walking chars of the original `body`.
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut match_char_idx = chars.len();
    for (i, (byte_idx, _)) in chars.iter().enumerate() {
        if *byte_idx >= match_byte_offset {
            match_char_idx = i;
            break;
        }
    }
    let start = match_char_idx.saturating_sub(60);
    let end = (match_char_idx + 60).min(chars.len());
    let raw: String = chars[start..end].iter().map(|(_, c)| *c).collect();
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

impl LocalSearchRepository for SqliteLocalSearchRepository {
    fn search(
        &self,
        query: &str,
        in_content: bool,
        limit: usize,
        body_loader: &dyn Fn(Uuid) -> Option<String>,
    ) -> Result<Vec<SearchHit>, StoreError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.store.conn()?;

        // Project hits — parameterised LIKE with COLLATE NOCASE. Result is
        // alphabetised by name.
        let mut project_stmt = conn.prepare(
            "SELECT id, name FROM local_project
             WHERE name LIKE '%' || ?1 || '%' COLLATE NOCASE
             ORDER BY name COLLATE NOCASE ASC",
        )?;
        let project_rows = project_stmt.query_map(params![trimmed], |row| {
            let id_text: String = row.get(0)?;
            let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
            let name: String = row.get(1)?;
            Ok((id, name))
        })?;
        let mut project_hits: Vec<SearchHit> = Vec::new();
        for r in project_rows {
            let (id, name) = r?;
            project_hits.push(SearchHit {
                kind: SearchKind::Project,
                id,
                project_id: None,
                title: name.clone(),
                breadcrumb: name,
                snippet: None,
                note_kind: None,
            });
        }

        // Note title hits, joined with their project for the breadcrumb. We
        // collect *all* candidate note rows (id, project_id, project_name,
        // title) — but only those whose title matches go into `note_title_hits`
        // here. For body matching below we additionally walk every note in the
        // store via the loader.
        let mut title_stmt = conn.prepare(
            "SELECT n.id, n.project_id, p.name, n.title, n.kind
             FROM local_note n
             JOIN local_project p ON p.id = n.project_id
             WHERE n.title LIKE '%' || ?1 || '%' COLLATE NOCASE
             ORDER BY n.title COLLATE NOCASE ASC",
        )?;
        let title_rows = title_stmt.query_map(params![trimmed], |row| {
            let id_text: String = row.get(0)?;
            let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
            let project_text: String = row.get(1)?;
            let project_id =
                Uuid::parse_str(&project_text).map_err(|_| invalid_uuid(project_text))?;
            Ok((
                id,
                project_id,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut note_hits: Vec<SearchHit> = Vec::new();
        let mut note_hit_ids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for r in title_rows {
            let (id, project_id, project_name, title, kind_str) = r?;
            note_hit_ids.insert(id);
            // Plans-Phase-9-wikilink-picker (rev 2): walk parent chain so
            // the breadcrumb shows `Project / Parent / Sub / Title`.
            let parents = parent_chain_titles(&conn, id);
            note_hits.push(SearchHit {
                kind: SearchKind::Note,
                id,
                project_id: Some(project_id),
                title: title.clone(),
                breadcrumb: compose_breadcrumb(&project_name, &parents, &title),
                snippet: None,
                note_kind: Some(NoteKind::from_str(&kind_str)),
            });
        }

        // Body matches. Walk every note (joined to its project for the
        // breadcrumb) and ask the closure for the body text. Skip notes already
        // matched by title (their snippet stays None — title hit semantics).
        if in_content {
            let needle_lower = trimmed.to_lowercase();
            let mut all_stmt = conn.prepare(
                "SELECT n.id, n.project_id, p.name, n.title, n.kind
                 FROM local_note n
                 JOIN local_project p ON p.id = n.project_id
                 ORDER BY n.title COLLATE NOCASE ASC",
            )?;
            let all_rows = all_stmt.query_map(params![], |row| {
                let id_text: String = row.get(0)?;
                let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
                let project_text: String = row.get(1)?;
                let project_id =
                    Uuid::parse_str(&project_text).map_err(|_| invalid_uuid(project_text))?;
                Ok((
                    id,
                    project_id,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            for r in all_rows {
                let (id, project_id, project_name, title, kind_str) = r?;
                if note_hit_ids.contains(&id) {
                    continue;
                }
                let Some(body) = body_loader(id) else {
                    continue;
                };
                let body_lower = body.to_lowercase();
                if let Some(byte_off) = body_lower.find(&needle_lower) {
                    let snippet = build_snippet(&body, byte_off);
                    let parents = parent_chain_titles(&conn, id);
                    note_hits.push(SearchHit {
                        kind: SearchKind::Note,
                        id,
                        project_id: Some(project_id),
                        title: title.clone(),
                        breadcrumb: compose_breadcrumb(&project_name, &parents, &title),
                        snippet: Some(snippet),
                        note_kind: Some(NoteKind::from_str(&kind_str)),
                    });
                    note_hit_ids.insert(id);
                }
            }
        }

        // Project hits first, then note hits. Each list is already sorted
        // alphabetical (case-insensitive) by SQL.
        let mut out = project_hits;
        out.extend(note_hits);
        let cap = limit.min(out.len());
        out.truncate(cap);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{
        LocalNoteRepository, LocalProjectRepository, SqliteLocalNoteRepository,
        SqliteLocalProjectRepository,
    };
    use crate::test_support::open_in_memory;

    fn empty_loader() -> Box<dyn Fn(Uuid) -> Option<String>> {
        Box::new(|_id: Uuid| None)
    }

    fn make_search() -> (
        SqliteLocalProjectRepository,
        SqliteLocalNoteRepository,
        SqliteLocalSearchRepository,
    ) {
        let store = open_in_memory().unwrap();
        let p = SqliteLocalProjectRepository::new(store.clone());
        let n = SqliteLocalNoteRepository::new(store.clone());
        let s = SqliteLocalSearchRepository::new(store);
        (p, n, s)
    }

    #[test]
    fn local_search_returns_empty_for_blank_query() {
        let (p, n, s) = make_search();
        let project = p.create("Alpha").unwrap();
        n.create(project.id, None, "First note").unwrap();
        let loader = empty_loader();
        let hits = s.search("", false, DEFAULT_SEARCH_LIMIT, &*loader).unwrap();
        assert!(hits.is_empty());
        let hits = s
            .search("   \t\n", false, DEFAULT_SEARCH_LIMIT, &*loader)
            .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn local_search_title_substring_case_insensitive() {
        let (p, n, s) = make_search();
        let project = p.create("Project").unwrap();
        let _alpha = n.create(project.id, None, "Alpha").unwrap();
        let _beta = n.create(project.id, None, "Beta").unwrap();
        let loader = empty_loader();
        let hits = s
            .search("al", false, DEFAULT_SEARCH_LIMIT, &*loader)
            .unwrap();
        // "al" matches the note title "Alpha" (case-insensitive).
        assert!(hits.iter().any(|h| h.title == "Alpha"));
        assert!(!hits.iter().any(|h| h.title == "Beta"));
    }

    #[test]
    fn local_search_returns_breadcrumb_for_note_hits() {
        let (p, n, s) = make_search();
        let project = p.create("Project1").unwrap();
        let _ = n.create(project.id, None, "Alpha").unwrap();
        let loader = empty_loader();
        let hits = s
            .search("Alpha", false, DEFAULT_SEARCH_LIMIT, &*loader)
            .unwrap();
        let note_hit = hits
            .iter()
            .find(|h| h.kind == SearchKind::Note)
            .expect("note hit");
        assert_eq!(note_hit.breadcrumb, "Project1 / Alpha");
        assert_eq!(note_hit.project_id, Some(project.id));
        // Plans-Phase-9-wikilink-picker (rev 1): default-kind notes
        // surface as `Markdown` so the picker can render `[md]` and
        // emit `[[…]]` syntax.
        assert_eq!(note_hit.note_kind, Some(NoteKind::Markdown));
    }

    #[test]
    fn local_search_breadcrumb_walks_parent_chain() {
        let (p, n, s) = make_search();
        let project = p.create("Jehu").unwrap();
        // Build a 3-level tree: Folder → Sub → Untitledkkk.
        let folder = n.create(project.id, None, "Folder").unwrap();
        let sub = n.create(project.id, Some(folder.id), "Sub").unwrap();
        let _leaf = n.create(project.id, Some(sub.id), "Untitledkkk").unwrap();
        // Also create a separate top-level note with the same title to
        // confirm the path discriminates between them.
        let _other = n.create(project.id, None, "Untitledkkk").unwrap();
        let loader = empty_loader();
        let hits = s
            .search("Untitledkkk", false, DEFAULT_SEARCH_LIMIT, &*loader)
            .unwrap();
        let nested = hits
            .iter()
            .find(|h| h.breadcrumb == "Jehu / Folder / Sub / Untitledkkk")
            .expect("nested breadcrumb");
        assert_eq!(nested.title, "Untitledkkk");
        let top = hits
            .iter()
            .find(|h| h.breadcrumb == "Jehu / Untitledkkk")
            .expect("top-level breadcrumb");
        assert_eq!(top.title, "Untitledkkk");
        assert_ne!(nested.id, top.id, "two distinct notes must surface");
    }

    #[test]
    fn local_search_emits_image_kind_for_image_notes() {
        let (p, n, s) = make_search();
        let project = p.create("Snapshots").unwrap();
        // Mint via create_with_kind so the row's kind column is `'image'`.
        n.create_with_kind(project.id, None, "Pic", NoteKind::Image)
            .unwrap();
        let loader = empty_loader();
        let hits = s.search("Pic", false, DEFAULT_SEARCH_LIMIT, &*loader).unwrap();
        let note_hit = hits
            .iter()
            .find(|h| h.kind == SearchKind::Note && h.title == "Pic")
            .expect("image note hit");
        assert_eq!(note_hit.note_kind, Some(NoteKind::Image));
        // Project hits never carry a NoteKind.
        let project_hits: Vec<_> = hits.iter().filter(|h| h.kind == SearchKind::Project).collect();
        for h in project_hits {
            assert!(h.note_kind.is_none(), "project hit must not carry a NoteKind");
        }
    }

    #[test]
    fn local_search_in_content_finds_body_match_with_snippet() {
        let (p, n, s) = make_search();
        let project = p.create("Notes").unwrap();
        let note = n.create(project.id, None, "TitleOnly").unwrap();
        let body = "intro text and the special_keyword shows here, then more after.";
        let note_id = note.id;
        let body_owned = body.to_string();
        let loader = move |id: Uuid| -> Option<String> {
            if id == note_id {
                Some(body_owned.clone())
            } else {
                None
            }
        };
        let hits = s
            .search("special_keyword", true, DEFAULT_SEARCH_LIMIT, &loader)
            .unwrap();
        let body_hit = hits.iter().find(|h| h.snippet.is_some()).expect("body hit");
        let snippet = body_hit.snippet.as_ref().unwrap();
        assert!(
            snippet.contains("special_keyword"),
            "snippet={snippet:?} body={body:?}"
        );
    }

    #[test]
    fn local_search_in_content_off_skips_body_only_matches() {
        let (p, n, s) = make_search();
        let project = p.create("Notes").unwrap();
        let _note = n.create(project.id, None, "TitleOnly").unwrap();
        let loader = |_id: Uuid| Some("hidden_word inside body".to_string());
        let hits = s
            .search("hidden_word", false, DEFAULT_SEARCH_LIMIT, &loader)
            .unwrap();
        assert!(
            hits.is_empty(),
            "in_content=false must not consult the body loader, got: {hits:?}"
        );
    }

    #[test]
    fn local_search_caps_at_limit() {
        let (p, n, s) = make_search();
        let project = p.create("Project").unwrap();
        for i in 0..250 {
            n.create(project.id, None, &format!("hit-{i:04}")).unwrap();
        }
        let loader = empty_loader();
        let hits = s.search("hit-", false, 200, &*loader).unwrap();
        assert_eq!(hits.len(), 200, "limit must cap output");
    }

    #[test]
    fn local_search_param_query_is_safe_against_sql_injection_attempts() {
        let (p, n, s) = make_search();
        let project = p.create("Project").unwrap();
        let _note = n.create(project.id, None, "First note").unwrap();
        let loader = empty_loader();

        // The injection attempt is treated as a literal LIKE substring — won't
        // match anything we have, and must NOT drop the table.
        let hits = s
            .search(
                "'; DROP TABLE local_note;",
                false,
                DEFAULT_SEARCH_LIMIT,
                &*loader,
            )
            .unwrap();
        assert!(hits.is_empty(), "injected query should yield no hits");

        // Table still exists: a follow-up legitimate query still works.
        let hits = s
            .search("First", false, DEFAULT_SEARCH_LIMIT, &*loader)
            .unwrap();
        assert!(hits.iter().any(|h| h.title == "First note"));
    }

    #[test]
    fn local_search_orders_projects_before_notes() {
        let (p, n, s) = make_search();
        let project_match = p.create("matchword-project").unwrap();
        let _ = n.create(project_match.id, None, "matchword-note").unwrap();
        let loader = empty_loader();
        let hits = s
            .search("matchword", false, DEFAULT_SEARCH_LIMIT, &*loader)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].kind, SearchKind::Project);
        assert_eq!(hits[1].kind, SearchKind::Note);
    }
}
