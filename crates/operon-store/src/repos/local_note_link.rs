//! Plans-Phase-5-vfs-wikilinks: read/write the wikilink graph.
//!
//! The save pipeline rebuilds rows for a single `source_note_id` whenever
//! its body changes; rename / delete propagation walks `referrers_of` and
//! rewrites raw text inside affected sources.

use rusqlite::params;
use uuid::Uuid;

use crate::error::StoreError;
use crate::sqlite::Store;

/// One row of the link graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRow {
    pub source_note_id: Uuid,
    pub target_text: String,
    pub target_note_id: Option<Uuid>,
    pub is_embed: bool,
}

pub trait LocalNoteLinkRepository: Send + Sync {
    /// Replace every link row for `source_note_id` atomically. The full
    /// previous set is removed and the new set inserted in one transaction.
    fn replace_for(&self, source_note_id: Uuid, links: &[LinkRow]) -> Result<(), StoreError>;

    /// All distinct source ids whose body references `target_note_id`.
    /// Used by rename / delete propagation.
    fn referrers_of(&self, target_note_id: Uuid) -> Result<Vec<Uuid>, StoreError>;

    /// Update rows whose `target_note_id` matches `target` to swap their
    /// `target_text`. Used when a target is renamed: we walk the table,
    /// rewriting the link's stored text in lockstep with the body text the
    /// caller just persisted.
    fn rewrite_target_text(
        &self,
        target_note_id: Uuid,
        old_text: &str,
        new_text: &str,
    ) -> Result<u64, StoreError>;

    /// All link rows whose `source_note_id` matches.
    fn list_for_source(&self, source_note_id: Uuid) -> Result<Vec<LinkRow>, StoreError>;
}

pub struct SqliteLocalNoteLinkRepository {
    store: Store,
}

impl SqliteLocalNoteLinkRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl LocalNoteLinkRepository for SqliteLocalNoteLinkRepository {
    fn replace_for(&self, source_note_id: Uuid, links: &[LinkRow]) -> Result<(), StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM local_note_link WHERE source_note_id = ?1",
            params![source_note_id.to_string()],
        )?;
        let mut stmt = tx.prepare(
            "INSERT INTO local_note_link (source_note_id, target_text, target_note_id, is_embed)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(source_note_id, target_text) DO UPDATE SET
                target_note_id = excluded.target_note_id,
                is_embed = excluded.is_embed",
        )?;
        for row in links {
            // Defensive: the row's source_note_id should match; we trust the
            // caller and use the parameter for consistency.
            stmt.execute(params![
                source_note_id.to_string(),
                row.target_text,
                row.target_note_id.map(|u| u.to_string()),
                if row.is_embed { 1 } else { 0 },
            ])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    fn referrers_of(&self, target_note_id: Uuid) -> Result<Vec<Uuid>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT source_note_id FROM local_note_link WHERE target_note_id = ?1",
        )?;
        let rows = stmt.query_map(params![target_note_id.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            let s = r?;
            if let Ok(id) = Uuid::parse_str(&s) {
                out.push(id);
            }
        }
        Ok(out)
    }

    fn rewrite_target_text(
        &self,
        target_note_id: Uuid,
        old_text: &str,
        new_text: &str,
    ) -> Result<u64, StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note_link SET target_text = ?3
             WHERE target_note_id = ?1 AND target_text = ?2",
            params![target_note_id.to_string(), old_text, new_text],
        )?;
        Ok(n as u64)
    }

    fn list_for_source(&self, source_note_id: Uuid) -> Result<Vec<LinkRow>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT source_note_id, target_text, target_note_id, is_embed
             FROM local_note_link WHERE source_note_id = ?1
             ORDER BY target_text",
        )?;
        let rows = stmt.query_map(params![source_note_id.to_string()], |row| {
            let src: String = row.get(0)?;
            let target_text: String = row.get(1)?;
            let target_id_opt: Option<String> = row.get(2)?;
            let is_embed_int: i64 = row.get(3)?;
            Ok((src, target_text, target_id_opt, is_embed_int))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (src, target_text, target_id_opt, is_embed_int) = r?;
            let source_note_id = Uuid::parse_str(&src).map_err(|e| {
                StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                ))
            })?;
            let target_note_id = match target_id_opt {
                Some(s) => Uuid::parse_str(&s).ok(),
                None => None,
            };
            out.push(LinkRow {
                source_note_id,
                target_text,
                target_note_id,
                is_embed: is_embed_int != 0,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{LocalNoteRepository, SqliteLocalNoteRepository, SqliteLocalProjectRepository, LocalProjectRepository};
    use crate::test_support::open_in_memory;

    fn fixture() -> (
        SqliteLocalNoteLinkRepository,
        SqliteLocalNoteRepository,
        SqliteLocalProjectRepository,
    ) {
        let store = open_in_memory().unwrap();
        let links = SqliteLocalNoteLinkRepository::new(store.clone());
        let notes = SqliteLocalNoteRepository::new(store.clone());
        let projects = SqliteLocalProjectRepository::new(store);
        (links, notes, projects)
    }

    #[test]
    fn replace_for_round_trip_and_referrers() {
        let (links, notes, projects) = fixture();
        let p = projects.create("P").unwrap();
        let a = notes.create(p.id, None, "A").unwrap();
        let b = notes.create(p.id, None, "B").unwrap();
        let c = notes.create(p.id, None, "C").unwrap();
        let initial = vec![
            LinkRow {
                source_note_id: a.id,
                target_text: "B".into(),
                target_note_id: Some(b.id),
                is_embed: false,
            },
            LinkRow {
                source_note_id: a.id,
                target_text: "C".into(),
                target_note_id: Some(c.id),
                is_embed: false,
            },
        ];
        links.replace_for(a.id, &initial).unwrap();
        let mut got_a = links.list_for_source(a.id).unwrap();
        got_a.sort_by(|x, y| x.target_text.cmp(&y.target_text));
        assert_eq!(got_a.len(), 2);
        assert_eq!(got_a[0].target_text, "B");
        assert_eq!(got_a[1].target_text, "C");

        let referrers_b = links.referrers_of(b.id).unwrap();
        assert_eq!(referrers_b, vec![a.id]);

        // Replace with a smaller set; old C entry is wiped.
        let next = vec![LinkRow {
            source_note_id: a.id,
            target_text: "B".into(),
            target_note_id: Some(b.id),
            is_embed: false,
        }];
        links.replace_for(a.id, &next).unwrap();
        let got = links.list_for_source(a.id).unwrap();
        assert_eq!(got.len(), 1);
        assert!(links.referrers_of(c.id).unwrap().is_empty());
    }

    #[test]
    fn rewrite_target_text_swaps_only_matching_rows() {
        let (links, notes, projects) = fixture();
        let p = projects.create("P").unwrap();
        let a = notes.create(p.id, None, "A").unwrap();
        let b = notes.create(p.id, None, "B").unwrap();
        let initial = vec![
            LinkRow {
                source_note_id: a.id,
                target_text: "B".into(),
                target_note_id: Some(b.id),
                is_embed: false,
            },
            LinkRow {
                source_note_id: a.id,
                target_text: "P/B".into(),
                target_note_id: Some(b.id),
                is_embed: false,
            },
        ];
        links.replace_for(a.id, &initial).unwrap();
        let n = links.rewrite_target_text(b.id, "B", "Bee").unwrap();
        assert_eq!(n, 1);
        let mut got = links.list_for_source(a.id).unwrap();
        got.sort_by(|x, y| x.target_text.cmp(&y.target_text));
        assert_eq!(got[0].target_text, "Bee");
        assert_eq!(got[1].target_text, "P/B");
    }

    #[test]
    fn target_set_null_on_target_delete() {
        let (links, notes, projects) = fixture();
        let p = projects.create("P").unwrap();
        let a = notes.create(p.id, None, "A").unwrap();
        let b = notes.create(p.id, None, "B").unwrap();
        links
            .replace_for(
                a.id,
                &[LinkRow {
                    source_note_id: a.id,
                    target_text: "B".into(),
                    target_note_id: Some(b.id),
                    is_embed: false,
                }],
            )
            .unwrap();
        // Sanity: PRAGMA foreign_keys must be ON for the SET NULL action; the
        // store enables it on open. After deleting B, the row remains but the
        // FK column drops to NULL — renderer treats that as broken.
        notes.delete(b.id).unwrap();
        let got = links.list_for_source(a.id).unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].target_note_id.is_none());
    }
}
