//! Local-mode note metadata. Backed by `local_note` (a sidecar to `local_project`).
//! Note content is stored in the Loro engine via `operon-notes`; this table only
//! holds the tree shape (parent/sibling/depth) and rename/timestamps.

use std::collections::HashMap;

use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StoreError;
use crate::store::Store;
use crate::time::now_ms;

const DEFAULT_NOTE_TITLE: &str = "Untitled";

/// The note's content kind. Backed by the `local_note.kind` column. The
/// allowed string set is enforced by a CHECK constraint at the SQL layer
/// (migrations 008, 011, 015) — adding a new variant here requires a new
/// migration that broadens the CHECK.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NoteKind {
    Markdown,
    Mdx,
    Image,
    Canvas,
    Excalidraw,
    Kanban,
    Code,
    /// M2: a Claude Code skill authored as a note. Body is markdown with
    /// optional YAML frontmatter (skill_name, skill_version, inputs,
    /// output_frontmatter). Materialized to `<repo>/.claude/skills/`
    /// on Play so claude's native skill loader resolves it.
    Skill,
    /// M3 preview: a workflow DAG of skill-node references. Stored at
    /// the SQL level for now; the React Flow editor lands in M3.
    Workflow,
}

impl NoteKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Mdx => "mdx",
            Self::Image => "image",
            Self::Canvas => "canvas",
            Self::Excalidraw => "excalidraw",
            Self::Kanban => "kanban",
            Self::Code => "code",
            Self::Skill => "skill",
            Self::Workflow => "workflow",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "mdx" => Self::Mdx,
            "image" => Self::Image,
            "canvas" => Self::Canvas,
            "excalidraw" => Self::Excalidraw,
            "kanban" => Self::Kanban,
            "code" => Self::Code,
            "skill" => Self::Skill,
            "workflow" => Self::Workflow,
            _ => Self::Markdown,
        }
    }

    /// Stable identifier the editor host uses to look up a `FormatPlugin`
    /// in `PluginRegistry`. Keep in sync with the `format_id` returned by
    /// each plugin's `manifest()`.
    pub fn format_id(&self) -> &'static str {
        self.as_str()
    }

    /// Human-readable label for the explorer's + dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Mdx => "MDX",
            Self::Image => "Image",
            Self::Canvas => "Canvas",
            Self::Excalidraw => "Excalidraw",
            Self::Kanban => "Kanban",
            Self::Code => "Code",
            Self::Skill => "Skill",
            Self::Workflow => "Workflow",
        }
    }

    /// Single-character glyph used by the explorer to badge each note row.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Mdx => "mx",
            Self::Image => "im",
            Self::Canvas => "cv",
            Self::Excalidraw => "ex",
            Self::Kanban => "kb",
            Self::Code => "{}",
            Self::Skill => "sk",
            Self::Workflow => "wf",
        }
    }

    /// Variants the user can pick from the explorer's + dropdown, in the
    /// order they should appear. Drives `project_row` and `note_row` so a
    /// future variant lights up everywhere by editing this list.
    pub fn all_creatable() -> &'static [NoteKind] {
        &[
            Self::Markdown,
            Self::Mdx,
            Self::Code,
            Self::Skill,
            Self::Image,
            Self::Kanban,
            Self::Canvas,
            Self::Excalidraw,
        ]
    }
}

impl Default for NoteKind {
    fn default() -> Self {
        Self::Markdown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalNote {
    pub id: Uuid,
    pub project_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub sibling_index: i64,
    pub depth: i64,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Plans-Phase-6-image-notes: defaults to `Markdown` for rows written
    /// before migration 008 (the column gets `'markdown'` via SQL default).
    #[serde(default)]
    pub kind: NoteKind,
    /// Plans-Phase-6-image-notes: vault-relative path to this note's image
    /// blob. `None` for markdown notes.
    #[serde(default)]
    pub blob_path: Option<String>,
}

pub trait LocalNoteRepository: Send + Sync {
    fn list_for_project(&self, project_id: Uuid) -> Result<Vec<LocalNote>, StoreError>;
    /// Single-row lookup for a note's `project_id`. Used by the companion
    /// rail to resolve "which project does this note belong to?" without
    /// loading every project's full note list.
    fn find_project_for_note(&self, _note_id: Uuid) -> Result<Option<Uuid>, StoreError> {
        // Default impl: linear scan. SQLite override below is O(1) via the
        // primary key. Trait default keeps non-SQLite implementors working.
        Err(StoreError::InvalidArgument(
            "find_project_for_note not implemented for this repo".into(),
        ))
    }
    fn create(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
    ) -> Result<LocalNote, StoreError>;

    /// Plans-Phase-6-image-notes: create a note with an explicit
    /// [`NoteKind`]. Default impl writes via `create` with `'markdown'`
    /// then patches the row's `kind` for `Image`. Sqlite impls override
    /// this for atomicity — the default just keeps every existing trait
    /// implementor working.
    fn create_with_kind(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
        kind: NoteKind,
    ) -> Result<LocalNote, StoreError> {
        let mut row = self.create(project_id, parent_id, title)?;
        if !matches!(kind, NoteKind::Markdown) {
            // The default body via `create` always lands as 'markdown'.
            // Implementors that override this method can write the
            // correct value in a single INSERT.
            self.set_kind(row.id, kind)?;
            row.kind = kind;
        }
        Ok(row)
    }

    /// Patch an existing note's [`NoteKind`].
    fn set_kind(&self, id: Uuid, kind: NoteKind) -> Result<(), StoreError>;

    /// Plans-Phase-6-image-notes: store the vault-relative path to the
    /// note's image blob. Pass `None` to clear.
    fn set_blob_path(&self, id: Uuid, path: Option<&str>) -> Result<(), StoreError>;

    fn rename(&self, id: Uuid, title: &str) -> Result<(), StoreError>;
    fn delete(&self, id: Uuid) -> Result<(), StoreError>;
    fn touch_updated(&self, id: Uuid) -> Result<(), StoreError>;

    /// Move `id` to `(new_project_id, new_parent, new_sibling_index)`. The whole
    /// subtree (descendants) follows. Sibling indexes in the destination are
    /// shifted to keep the dense ordering invariant. Atomic in a single tx.
    /// Returns `InvalidArgument` if the move would create a cycle (target is
    /// the node itself or one of its descendants).
    fn move_to(
        &self,
        id: Uuid,
        new_project_id: Uuid,
        new_parent: Option<Uuid>,
        new_sibling_index: i64,
    ) -> Result<(), StoreError>;

    /// Deep-copy the subtree rooted at `id` into
    /// `(into_project, into_parent, new_sibling_index)`. New UUIDs are minted
    /// for every node; structure (depth, sibling_index relative to siblings,
    /// parent links) is preserved. Atomic in a single tx. Returns the new
    /// root's id.
    fn duplicate_subtree(
        &self,
        id: Uuid,
        into_project: Uuid,
        into_parent: Option<Uuid>,
        new_sibling_index: i64,
    ) -> Result<Uuid, StoreError>;

    /// Reparent `id` so it becomes the last child of its previous sibling.
    /// No-op when the node is already the first sibling at its level.
    fn indent(&self, id: Uuid) -> Result<(), StoreError>;

    /// Reparent `id` to its grandparent (or to project root when
    /// grandparent is `None`), placing it immediately after the old parent.
    /// No-op when the node is already at depth 0.
    fn outdent(&self, id: Uuid) -> Result<(), StoreError>;

    /// Swap `sibling_index` with the previous sibling at the same level.
    /// No-op at the first sibling.
    fn move_up(&self, id: Uuid) -> Result<(), StoreError>;

    /// Swap `sibling_index` with the next sibling at the same level.
    /// No-op at the last sibling.
    fn move_down(&self, id: Uuid) -> Result<(), StoreError>;

    /// Plans-Phase-8-explorer-undo: capture the full subtree rooted at `id`
    /// into a snapshot suitable for `restore_subtree`. The snapshot lists
    /// every note with its current parent_id / sibling_index / depth /
    /// title / created_at_ms / kind / blob_path so a subsequent
    /// `restore_subtree` reproduces exactly what was on disk before delete.
    /// Returns `NotFound` if `id` doesn't exist.
    fn snapshot_subtree(&self, id: Uuid) -> Result<SubtreeSnapshot, StoreError>;

    /// Plans-Phase-8-explorer-undo: re-insert a snapshot previously captured
    /// via `snapshot_subtree`. Walks the snapshot in BFS order so parents
    /// land before their children, satisfying the FK on `parent_id`. Sibling
    /// indices around the destination are densified the same way `move_to`
    /// does. Returns `Conflict` if any of the snapshot's ids already exist.
    fn restore_subtree(&self, snap: &SubtreeSnapshot) -> Result<(), StoreError>;
}

/// Plans-Phase-8-explorer-undo: stable, in-memory representation of a
/// subtree captured by `snapshot_subtree`. Lives next to the trait so the
/// type can be passed around without an explicit `LocalNote` Vec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeSnapshot {
    pub root_id: Uuid,
    /// Notes in BFS order — root first, then each level. `restore_subtree`
    /// can walk this list once and INSERT in order; no re-sorting needed.
    pub notes: Vec<LocalNote>,
}

pub struct SqliteLocalNoteRepository {
    store: Store,
}

impl SqliteLocalNoteRepository {
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

fn row_to_local_note(row: &crate::sql::Row<'_>) -> crate::sql::Result<LocalNote> {
    let id_text: String = row.get(0)?;
    let id = Uuid::parse_str(&id_text).map_err(|_| invalid_uuid(id_text))?;
    let project_text: String = row.get(1)?;
    let project_id = Uuid::parse_str(&project_text).map_err(|_| invalid_uuid(project_text))?;
    let parent_opt: Option<String> = row.get(2)?;
    let parent_id = match parent_opt {
        Some(s) => Some(Uuid::parse_str(&s).map_err(|_| invalid_uuid(s))?),
        None => None,
    };
    // Plans-Phase-6-image-notes: column 8 is `kind`; rows from before
    // migration 008 don't have it, but the migration adds the column with
    // `DEFAULT 'markdown'` so every row reads as a string post-migrate. We
    // tolerate older queries that might not select the column by treating
    // a missing column as Markdown.
    let kind: NoteKind = row
        .get::<_, String>(8)
        .map(|s| NoteKind::from_str(&s))
        .unwrap_or_default();
    let blob_path: Option<String> = row.get(9).unwrap_or(None);
    Ok(LocalNote {
        id,
        project_id,
        parent_id,
        sibling_index: row.get(3)?,
        depth: row.get(4)?,
        title: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
        kind,
        blob_path,
    })
}

const SELECT_COLS: &str =
    "id, project_id, parent_id, sibling_index, depth, title, created_at_ms, updated_at_ms, kind, blob_path";

impl LocalNoteRepository for SqliteLocalNoteRepository {
    fn list_for_project(&self, project_id: Uuid) -> Result<Vec<LocalNote>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM local_note
             WHERE project_id = ?1
             ORDER BY parent_id IS NULL DESC, parent_id, sibling_index, created_at_ms"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id.to_string()], row_to_local_note)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn find_project_for_note(&self, note_id: Uuid) -> Result<Option<Uuid>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt =
            conn.prepare("SELECT project_id FROM local_note WHERE id = ?1")?;
        let project_id_text: Option<String> = stmt
            .query_row(params![note_id.to_string()], |row| row.get::<_, String>(0))
            .optional()?;
        match project_id_text {
            Some(s) => Uuid::parse_str(&s)
                .map(Some)
                .map_err(|_| StoreError::InvalidArgument(format!("invalid uuid: {s}"))),
            None => Ok(None),
        }
    }

    fn create(
        &self,
        project_id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
    ) -> Result<LocalNote, StoreError> {
        let trimmed = title.trim();
        let resolved_title = if trimmed.is_empty() {
            DEFAULT_NOTE_TITLE
        } else {
            trimmed
        };
        let id = Uuid::new_v4();
        let now = now_ms();
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;

        // Validate parent (when given) belongs to the same project, and derive depth.
        let depth = match parent_id {
            Some(pid) => {
                let parent_row: Option<(String, i64)> = tx
                    .query_row(
                        "SELECT project_id, depth FROM local_note WHERE id = ?1",
                        params![pid.to_string()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?;
                let (parent_project, parent_depth) = parent_row.ok_or_else(|| {
                    StoreError::InvalidArgument(format!("parent note {pid} not found"))
                })?;
                let parent_project_uuid = Uuid::parse_str(&parent_project).map_err(|_| {
                    StoreError::InvalidArgument(format!(
                        "stored parent project_id is not a uuid: {parent_project}"
                    ))
                })?;
                if parent_project_uuid != project_id {
                    return Err(StoreError::InvalidArgument(
                        "parent note belongs to a different project".into(),
                    ));
                }
                parent_depth + 1
            }
            None => 0,
        };

        let next_index: i64 = match parent_id {
            Some(pid) => tx.query_row(
                "SELECT COALESCE(MAX(sibling_index), -1) + 1 FROM local_note
                 WHERE project_id = ?1 AND parent_id = ?2",
                params![project_id.to_string(), pid.to_string()],
                |row| row.get(0),
            )?,
            None => tx.query_row(
                "SELECT COALESCE(MAX(sibling_index), -1) + 1 FROM local_note
                 WHERE project_id = ?1 AND parent_id IS NULL",
                params![project_id.to_string()],
                |row| row.get(0),
            )?,
        };

        tx.execute(
            "INSERT INTO local_note (id, project_id, parent_id, sibling_index, depth,
                                     title, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id.to_string(),
                project_id.to_string(),
                parent_id.map(|p| p.to_string()),
                next_index,
                depth,
                resolved_title,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(LocalNote {
            id,
            project_id,
            parent_id,
            sibling_index: next_index,
            depth,
            title: resolved_title.to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            kind: NoteKind::Markdown,
            blob_path: None,
        })
    }

    fn rename(&self, id: Uuid, title: &str) -> Result<(), StoreError> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidArgument(
                "note title must not be empty or whitespace-only".into(),
            ));
        }
        let now = now_ms();
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note SET title = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), trimmed, now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn set_kind(&self, id: Uuid, kind: NoteKind) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note SET kind = ?2 WHERE id = ?1",
            params![id.to_string(), kind.as_str()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn set_blob_path(&self, id: Uuid, path: Option<&str>) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note SET blob_path = ?2 WHERE id = ?1",
            params![id.to_string(), path],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        // ON DELETE CASCADE handles descendants.
        conn.execute(
            "DELETE FROM local_note WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn touch_updated(&self, id: Uuid) -> Result<(), StoreError> {
        let now = now_ms();
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE local_note SET updated_at_ms = ?2 WHERE id = ?1",
            params![id.to_string(), now],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn move_to(
        &self,
        id: Uuid,
        new_project_id: Uuid,
        new_parent: Option<Uuid>,
        new_sibling_index: i64,
    ) -> Result<(), StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;

        // Load the moved node.
        let node_row: Option<(String, Option<String>, i64, i64)> = tx
            .query_row(
                "SELECT project_id, parent_id, sibling_index, depth FROM local_note WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((old_project_text, old_parent_text, old_sibling_index, old_depth)) = node_row
        else {
            return Err(StoreError::NotFound);
        };
        let old_project = Uuid::parse_str(&old_project_text).map_err(|_| {
            StoreError::InvalidArgument(format!("invalid uuid: {old_project_text}"))
        })?;
        let old_parent = match old_parent_text {
            Some(s) => Some(
                Uuid::parse_str(&s)
                    .map_err(|_| StoreError::InvalidArgument(format!("invalid uuid: {s}")))?,
            ),
            None => None,
        };

        // Reject moving onto self.
        if new_parent == Some(id) {
            return Err(StoreError::InvalidArgument(
                "cannot move a note inside itself".into(),
            ));
        }

        // Validate the new parent (when set) belongs to the destination project,
        // and detect cycle (target is a descendant of the moved node).
        let new_parent_depth = match new_parent {
            Some(npid) => {
                let parent_row: Option<(String, i64)> = tx
                    .query_row(
                        "SELECT project_id, depth FROM local_note WHERE id = ?1",
                        params![npid.to_string()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?;
                let (parent_project_text, parent_depth) = parent_row.ok_or_else(|| {
                    StoreError::InvalidArgument(format!("parent note {npid} not found"))
                })?;
                let parent_project_uuid = Uuid::parse_str(&parent_project_text)
                    .map_err(|_| invalid_uuid(parent_project_text))?;
                if parent_project_uuid != new_project_id {
                    return Err(StoreError::InvalidArgument(
                        "parent note belongs to a different project".into(),
                    ));
                }
                // Cycle check: walk up from new_parent through ancestors. If we
                // ever hit `id`, the move would be cyclic.
                let mut cursor = Some(npid);
                while let Some(c) = cursor {
                    if c == id {
                        return Err(StoreError::InvalidArgument(
                            "cannot move a note into one of its descendants".into(),
                        ));
                    }
                    let next: Option<String> = tx
                        .query_row(
                            "SELECT parent_id FROM local_note WHERE id = ?1",
                            params![c.to_string()],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .optional()?
                        .flatten();
                    cursor = match next {
                        Some(s) => Some(Uuid::parse_str(&s).map_err(|_| {
                            StoreError::InvalidArgument(format!("invalid uuid: {s}"))
                        })?),
                        None => None,
                    };
                }
                parent_depth
            }
            None => -1,
        };
        let new_depth = new_parent_depth + 1;

        // Sibling-index shifts are split based on whether the move stays inside
        // the same (project, parent) bucket.
        let same_bucket = old_project == new_project_id && old_parent == new_parent;
        let now = now_ms();

        // Compute the dest count BEFORE removing the source slot in same-bucket cases.
        let dest_count: i64 = match new_parent {
            Some(npid) => tx.query_row(
                "SELECT COUNT(*) FROM local_note
                 WHERE project_id = ?1 AND parent_id = ?2",
                params![new_project_id.to_string(), npid.to_string()],
                |row| row.get(0),
            )?,
            None => tx.query_row(
                "SELECT COUNT(*) FROM local_note
                 WHERE project_id = ?1 AND parent_id IS NULL",
                params![new_project_id.to_string()],
                |row| row.get(0),
            )?,
        };

        if same_bucket {
            let max_target = dest_count.saturating_sub(1);
            let target = new_sibling_index.clamp(0, max_target);
            if target != old_sibling_index {
                if target > old_sibling_index {
                    // Shift (old, target] up by one.
                    match new_parent {
                        Some(npid) => {
                            tx.execute(
                                "UPDATE local_note SET sibling_index = sibling_index - 1
                                 WHERE project_id = ?1 AND parent_id = ?2
                                   AND sibling_index > ?3 AND sibling_index <= ?4",
                                params![
                                    new_project_id.to_string(),
                                    npid.to_string(),
                                    old_sibling_index,
                                    target
                                ],
                            )?;
                        }
                        None => {
                            tx.execute(
                                "UPDATE local_note SET sibling_index = sibling_index - 1
                                 WHERE project_id = ?1 AND parent_id IS NULL
                                   AND sibling_index > ?2 AND sibling_index <= ?3",
                                params![new_project_id.to_string(), old_sibling_index, target],
                            )?;
                        }
                    }
                } else {
                    // Shift [target, old) down by one.
                    match new_parent {
                        Some(npid) => {
                            tx.execute(
                                "UPDATE local_note SET sibling_index = sibling_index + 1
                                 WHERE project_id = ?1 AND parent_id = ?2
                                   AND sibling_index >= ?3 AND sibling_index < ?4",
                                params![
                                    new_project_id.to_string(),
                                    npid.to_string(),
                                    target,
                                    old_sibling_index
                                ],
                            )?;
                        }
                        None => {
                            tx.execute(
                                "UPDATE local_note SET sibling_index = sibling_index + 1
                                 WHERE project_id = ?1 AND parent_id IS NULL
                                   AND sibling_index >= ?2 AND sibling_index < ?3",
                                params![new_project_id.to_string(), target, old_sibling_index],
                            )?;
                        }
                    }
                }
                tx.execute(
                    "UPDATE local_note SET sibling_index = ?2, updated_at_ms = ?3 WHERE id = ?1",
                    params![id.to_string(), target, now],
                )?;
            }
        } else {
            // Cross-bucket move. Close the gap in the source bucket; open a slot
            // in the destination bucket.
            match old_parent {
                Some(opid) => {
                    tx.execute(
                        "UPDATE local_note SET sibling_index = sibling_index - 1
                         WHERE project_id = ?1 AND parent_id = ?2
                           AND sibling_index > ?3",
                        params![old_project.to_string(), opid.to_string(), old_sibling_index],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE local_note SET sibling_index = sibling_index - 1
                         WHERE project_id = ?1 AND parent_id IS NULL
                           AND sibling_index > ?2",
                        params![old_project.to_string(), old_sibling_index],
                    )?;
                }
            }

            let max_target = dest_count;
            let target = new_sibling_index.clamp(0, max_target);
            match new_parent {
                Some(npid) => {
                    tx.execute(
                        "UPDATE local_note SET sibling_index = sibling_index + 1
                         WHERE project_id = ?1 AND parent_id = ?2
                           AND sibling_index >= ?3",
                        params![new_project_id.to_string(), npid.to_string(), target],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE local_note SET sibling_index = sibling_index + 1
                         WHERE project_id = ?1 AND parent_id IS NULL
                           AND sibling_index >= ?2",
                        params![new_project_id.to_string(), target],
                    )?;
                }
            }

            // Update the moved node itself.
            tx.execute(
                "UPDATE local_note
                 SET project_id = ?2, parent_id = ?3, sibling_index = ?4,
                     depth = ?5, updated_at_ms = ?6
                 WHERE id = ?1",
                params![
                    id.to_string(),
                    new_project_id.to_string(),
                    new_parent.map(|p| p.to_string()),
                    target,
                    new_depth,
                    now,
                ],
            )?;

            // Recompute depth + project_id for descendants. delta = new_depth - old_depth.
            let delta = new_depth - old_depth;
            // Walk the subtree breadth-first, collecting ids.
            let mut ids: Vec<String> = Vec::new();
            let mut frontier: Vec<String> = vec![id.to_string()];
            while let Some(parent_text) = frontier.pop() {
                let mut stmt = tx.prepare(
                    "SELECT id FROM local_note
                     WHERE parent_id = ?1",
                )?;
                let rows = stmt.query_map(params![parent_text], |row| row.get::<_, String>(0))?;
                for r in rows {
                    let child = r?;
                    ids.push(child.clone());
                    frontier.push(child);
                }
            }
            for child_id in ids {
                tx.execute(
                    "UPDATE local_note SET project_id = ?2, depth = depth + ?3, updated_at_ms = ?4
                     WHERE id = ?1",
                    params![child_id, new_project_id.to_string(), delta, now],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    fn duplicate_subtree(
        &self,
        id: Uuid,
        into_project: Uuid,
        into_parent: Option<Uuid>,
        new_sibling_index: i64,
    ) -> Result<Uuid, StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;

        // Validate destination parent (when set).
        let dest_parent_depth = match into_parent {
            Some(pid) => {
                let parent_row: Option<(String, i64)> = tx
                    .query_row(
                        "SELECT project_id, depth FROM local_note WHERE id = ?1",
                        params![pid.to_string()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?;
                let (parent_project_text, parent_depth) = parent_row.ok_or_else(|| {
                    StoreError::InvalidArgument(format!("parent note {pid} not found"))
                })?;
                let parent_project_uuid = Uuid::parse_str(&parent_project_text)
                    .map_err(|_| invalid_uuid(parent_project_text))?;
                if parent_project_uuid != into_project {
                    return Err(StoreError::InvalidArgument(
                        "parent note belongs to a different project".into(),
                    ));
                }
                parent_depth
            }
            None => -1,
        };

        // Load the source root.
        let source_row: Option<(i64, String)> = tx
            .query_row(
                "SELECT depth, title FROM local_note WHERE id = ?1",
                params![id.to_string()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((source_depth, source_title)) = source_row else {
            return Err(StoreError::NotFound);
        };

        // Open a slot in destination bucket.
        let dest_count: i64 = match into_parent {
            Some(pid) => tx.query_row(
                "SELECT COUNT(*) FROM local_note
                 WHERE project_id = ?1 AND parent_id = ?2",
                params![into_project.to_string(), pid.to_string()],
                |row| row.get(0),
            )?,
            None => tx.query_row(
                "SELECT COUNT(*) FROM local_note
                 WHERE project_id = ?1 AND parent_id IS NULL",
                params![into_project.to_string()],
                |row| row.get(0),
            )?,
        };
        let target = new_sibling_index.clamp(0, dest_count);
        match into_parent {
            Some(pid) => {
                tx.execute(
                    "UPDATE local_note SET sibling_index = sibling_index + 1
                     WHERE project_id = ?1 AND parent_id = ?2 AND sibling_index >= ?3",
                    params![into_project.to_string(), pid.to_string(), target],
                )?;
            }
            None => {
                tx.execute(
                    "UPDATE local_note SET sibling_index = sibling_index + 1
                     WHERE project_id = ?1 AND parent_id IS NULL AND sibling_index >= ?2",
                    params![into_project.to_string(), target],
                )?;
            }
        }

        let now = now_ms();
        let new_root_id = Uuid::new_v4();
        let new_root_depth = dest_parent_depth + 1;
        tx.execute(
            "INSERT INTO local_note (id, project_id, parent_id, sibling_index, depth,
                                     title, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                new_root_id.to_string(),
                into_project.to_string(),
                into_parent.map(|p| p.to_string()),
                target,
                new_root_depth,
                source_title,
                now,
            ],
        )?;

        // BFS over source descendants, mapping original id -> new id, copying
        // them in order of (parent, sibling_index).
        let mut id_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        id_map.insert(id.to_string(), new_root_id.to_string());
        let depth_delta = new_root_depth - source_depth;
        let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        queue.push_back(id.to_string());
        while let Some(parent_text) = queue.pop_front() {
            let new_parent_text = id_map.get(&parent_text).cloned().expect("parent mapped");
            let mut stmt = tx.prepare(
                "SELECT id, sibling_index, depth, title FROM local_note
                 WHERE parent_id = ?1
                 ORDER BY sibling_index ASC",
            )?;
            let rows = stmt.query_map(params![parent_text], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            // Materialise so we can re-borrow tx for inserts.
            let collected: Vec<(String, i64, i64, String)> = rows.collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            for (child_old_id, child_sibling, child_depth, child_title) in collected {
                let child_new_id = Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO local_note (id, project_id, parent_id, sibling_index, depth,
                                             title, created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    params![
                        child_new_id,
                        into_project.to_string(),
                        new_parent_text,
                        child_sibling,
                        child_depth + depth_delta,
                        child_title,
                        now,
                    ],
                )?;
                id_map.insert(child_old_id.clone(), child_new_id);
                queue.push_back(child_old_id);
            }
        }

        tx.commit()?;
        Ok(new_root_id)
    }

    fn indent(&self, id: Uuid) -> Result<(), StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        let row: Option<(String, Option<String>, i64, i64)> = tx
            .query_row(
                "SELECT project_id, parent_id, sibling_index, depth FROM local_note WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((project_text, parent_text, sibling_index, old_depth)) = row else {
            return Err(StoreError::NotFound);
        };
        if sibling_index == 0 {
            tx.commit()?;
            return Ok(());
        }
        let project_id = Uuid::parse_str(&project_text).map_err(|_| invalid_uuid(project_text))?;

        // Find the previous sibling.
        let prev_sibling: Option<String> = match &parent_text {
            Some(pt) => tx
                .query_row(
                    "SELECT id FROM local_note
                     WHERE project_id = ?1 AND parent_id = ?2 AND sibling_index = ?3",
                    params![project_id.to_string(), pt, sibling_index - 1],
                    |row| row.get::<_, String>(0),
                )
                .optional()?,
            None => tx
                .query_row(
                    "SELECT id FROM local_note
                     WHERE project_id = ?1 AND parent_id IS NULL AND sibling_index = ?2",
                    params![project_id.to_string(), sibling_index - 1],
                    |row| row.get::<_, String>(0),
                )
                .optional()?,
        };
        let Some(prev_sibling_text) = prev_sibling else {
            tx.commit()?;
            return Ok(());
        };
        let prev_sibling_id =
            Uuid::parse_str(&prev_sibling_text).map_err(|_| invalid_uuid(prev_sibling_text))?;

        // Compute new sibling_index = max child index of new parent + 1.
        let next_index: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sibling_index), -1) + 1 FROM local_note
             WHERE project_id = ?1 AND parent_id = ?2",
            params![project_id.to_string(), prev_sibling_id.to_string()],
            |row| row.get(0),
        )?;

        // Close gap in old bucket (everything after `sibling_index`).
        match &parent_text {
            Some(pt) => {
                tx.execute(
                    "UPDATE local_note SET sibling_index = sibling_index - 1
                     WHERE project_id = ?1 AND parent_id = ?2 AND sibling_index > ?3",
                    params![project_id.to_string(), pt, sibling_index],
                )?;
            }
            None => {
                tx.execute(
                    "UPDATE local_note SET sibling_index = sibling_index - 1
                     WHERE project_id = ?1 AND parent_id IS NULL AND sibling_index > ?2",
                    params![project_id.to_string(), sibling_index],
                )?;
            }
        }

        let now = now_ms();
        let new_depth = old_depth + 1;
        let depth_delta = new_depth - old_depth;
        tx.execute(
            "UPDATE local_note
             SET parent_id = ?2, sibling_index = ?3, depth = ?4, updated_at_ms = ?5
             WHERE id = ?1",
            params![
                id.to_string(),
                prev_sibling_id.to_string(),
                next_index,
                new_depth,
                now
            ],
        )?;

        // Recompute descendant depths (delta = +1).
        if depth_delta != 0 {
            let mut frontier: Vec<String> = vec![id.to_string()];
            let mut to_update: Vec<String> = Vec::new();
            while let Some(parent_text) = frontier.pop() {
                let mut stmt = tx.prepare("SELECT id FROM local_note WHERE parent_id = ?1")?;
                let rows = stmt.query_map(params![parent_text], |row| row.get::<_, String>(0))?;
                for r in rows {
                    let c = r?;
                    to_update.push(c.clone());
                    frontier.push(c);
                }
            }
            for child_id in to_update {
                tx.execute(
                    "UPDATE local_note SET depth = depth + ?2, updated_at_ms = ?3 WHERE id = ?1",
                    params![child_id, depth_delta, now],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    fn outdent(&self, id: Uuid) -> Result<(), StoreError> {
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        let row: Option<(String, Option<String>, i64, i64)> = tx
            .query_row(
                "SELECT project_id, parent_id, sibling_index, depth FROM local_note WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((project_text, parent_text, sibling_index, old_depth)) = row else {
            return Err(StoreError::NotFound);
        };
        if old_depth == 0 || parent_text.is_none() {
            tx.commit()?;
            return Ok(());
        }
        let project_id = Uuid::parse_str(&project_text).map_err(|_| invalid_uuid(project_text))?;
        let parent_text = parent_text.expect("checked above");
        let parent_id =
            Uuid::parse_str(&parent_text).map_err(|_| invalid_uuid(parent_text.clone()))?;

        // Read the parent to find the grandparent + parent's sibling_index.
        let parent_info: (Option<String>, i64) = tx.query_row(
            "SELECT parent_id, sibling_index FROM local_note WHERE id = ?1",
            params![parent_id.to_string()],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let (grandparent_text, parent_sibling_index) = parent_info;
        let grandparent_id = match grandparent_text.as_deref() {
            Some(s) => Some(Uuid::parse_str(s).map_err(|_| invalid_uuid(s.to_string()))?),
            None => None,
        };

        // Close gap in old (parent) bucket: everything after `sibling_index`.
        tx.execute(
            "UPDATE local_note SET sibling_index = sibling_index - 1
             WHERE project_id = ?1 AND parent_id = ?2 AND sibling_index > ?3",
            params![project_id.to_string(), parent_id.to_string(), sibling_index],
        )?;

        // Open slot at parent_sibling_index + 1 in grandparent's bucket.
        let target = parent_sibling_index + 1;
        match grandparent_id {
            Some(gpid) => {
                tx.execute(
                    "UPDATE local_note SET sibling_index = sibling_index + 1
                     WHERE project_id = ?1 AND parent_id = ?2 AND sibling_index >= ?3",
                    params![project_id.to_string(), gpid.to_string(), target],
                )?;
            }
            None => {
                tx.execute(
                    "UPDATE local_note SET sibling_index = sibling_index + 1
                     WHERE project_id = ?1 AND parent_id IS NULL AND sibling_index >= ?2",
                    params![project_id.to_string(), target],
                )?;
            }
        }

        let now = now_ms();
        let new_depth = old_depth - 1;
        let depth_delta = new_depth - old_depth;
        tx.execute(
            "UPDATE local_note
             SET parent_id = ?2, sibling_index = ?3, depth = ?4, updated_at_ms = ?5
             WHERE id = ?1",
            params![
                id.to_string(),
                grandparent_id.map(|g| g.to_string()),
                target,
                new_depth,
                now
            ],
        )?;

        // Recompute descendant depths (delta = -1).
        if depth_delta != 0 {
            let mut frontier: Vec<String> = vec![id.to_string()];
            let mut to_update: Vec<String> = Vec::new();
            while let Some(parent_text) = frontier.pop() {
                let mut stmt = tx.prepare("SELECT id FROM local_note WHERE parent_id = ?1")?;
                let rows = stmt.query_map(params![parent_text], |row| row.get::<_, String>(0))?;
                for r in rows {
                    let c = r?;
                    to_update.push(c.clone());
                    frontier.push(c);
                }
            }
            for child_id in to_update {
                tx.execute(
                    "UPDATE local_note SET depth = depth + ?2, updated_at_ms = ?3 WHERE id = ?1",
                    params![child_id, depth_delta, now],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    fn move_up(&self, id: Uuid) -> Result<(), StoreError> {
        swap_with_neighbour(&self.store, id, -1)
    }

    fn move_down(&self, id: Uuid) -> Result<(), StoreError> {
        swap_with_neighbour(&self.store, id, 1)
    }

    fn snapshot_subtree(&self, id: Uuid) -> Result<SubtreeSnapshot, StoreError> {
        let conn = self.store.conn()?;
        // Confirm the root exists (returns NotFound otherwise — same shape
        // as `rename` / `delete` for missing rows). `optional()` maps the
        // "no rows" case to None on both desktop and wasm-sqlite backends.
        let root_opt: Option<LocalNote> = conn
            .query_row(
                &format!("SELECT {SELECT_COLS} FROM local_note WHERE id = ?1"),
                params![id.to_string()],
                row_to_local_note,
            )
            .optional()?;
        let Some(root) = root_opt else {
            return Err(StoreError::NotFound);
        };
        let mut notes: Vec<LocalNote> = vec![root];
        let mut frontier: Vec<Uuid> = vec![id];
        // BFS — stable order so the snapshot is deterministic.
        while let Some(parent) = frontier.pop() {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLS} FROM local_note \
                 WHERE parent_id = ?1 ORDER BY sibling_index ASC"
            ))?;
            let rows: Vec<LocalNote> = stmt
                .query_map(params![parent.to_string()], row_to_local_note)?
                .collect::<crate::sql::Result<Vec<_>>>()?;
            for child in rows {
                frontier.push(child.id);
                notes.push(child);
            }
        }
        Ok(SubtreeSnapshot { root_id: id, notes })
    }

    fn restore_subtree(&self, snap: &SubtreeSnapshot) -> Result<(), StoreError> {
        if snap.notes.is_empty() {
            return Ok(());
        }
        let mut conn = self.store.conn()?;
        let tx = conn.transaction()?;
        // Re-densify destination siblings: shift any siblings at the root's
        // original (project, parent, sibling_index) up by 1 to make room.
        // Same Some/None split as `move_to` for the parent_id IS NULL case.
        let root = &snap.notes[0];
        match root.parent_id {
            Some(pid) => {
                tx.execute(
                    "UPDATE local_note \
                     SET sibling_index = sibling_index + 1 \
                     WHERE project_id = ?1 AND parent_id = ?2 \
                       AND sibling_index >= ?3",
                    params![
                        root.project_id.to_string(),
                        pid.to_string(),
                        root.sibling_index,
                    ],
                )?;
            }
            None => {
                tx.execute(
                    "UPDATE local_note \
                     SET sibling_index = sibling_index + 1 \
                     WHERE project_id = ?1 AND parent_id IS NULL \
                       AND sibling_index >= ?2",
                    params![root.project_id.to_string(), root.sibling_index],
                )?;
            }
        }
        // BFS-ordered notes guarantee parents land before children, so the
        // FK on parent_id (ON DELETE CASCADE) doesn't trip.
        for n in &snap.notes {
            tx.execute(
                "INSERT INTO local_note \
                 (id, project_id, parent_id, sibling_index, depth, title, \
                  created_at_ms, updated_at_ms, kind, blob_path) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    n.id.to_string(),
                    n.project_id.to_string(),
                    n.parent_id.map(|p| p.to_string()),
                    n.sibling_index,
                    n.depth,
                    n.title,
                    n.created_at_ms,
                    n.updated_at_ms,
                    n.kind.as_str(),
                    n.blob_path,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn swap_with_neighbour(store: &Store, id: Uuid, dir: i64) -> Result<(), StoreError> {
    debug_assert!(dir == 1 || dir == -1);
    let mut conn = store.conn()?;
    let tx = conn.transaction()?;
    let row: Option<(String, Option<String>, i64)> = tx
        .query_row(
            "SELECT project_id, parent_id, sibling_index FROM local_note WHERE id = ?1",
            params![id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((project_text, parent_text, sibling_index)) = row else {
        return Err(StoreError::NotFound);
    };
    let target_index = sibling_index + dir;
    if target_index < 0 {
        tx.commit()?;
        return Ok(());
    }
    let neighbour: Option<String> = match &parent_text {
        Some(pt) => tx
            .query_row(
                "SELECT id FROM local_note
                 WHERE project_id = ?1 AND parent_id = ?2 AND sibling_index = ?3",
                params![project_text, pt, target_index],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
        None => tx
            .query_row(
                "SELECT id FROM local_note
                 WHERE project_id = ?1 AND parent_id IS NULL AND sibling_index = ?2",
                params![project_text, target_index],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
    };
    let Some(neighbour_text) = neighbour else {
        tx.commit()?;
        return Ok(());
    };
    let now = now_ms();
    // Two-step swap to avoid colliding on the (project_id, parent_id, sibling_index)
    // bucket — first stash the moved row at -1, then update neighbour, then place it.
    tx.execute(
        "UPDATE local_note SET sibling_index = -1, updated_at_ms = ?2 WHERE id = ?1",
        params![id.to_string(), now],
    )?;
    tx.execute(
        "UPDATE local_note SET sibling_index = ?2, updated_at_ms = ?3 WHERE id = ?1",
        params![neighbour_text, sibling_index, now],
    )?;
    tx.execute(
        "UPDATE local_note SET sibling_index = ?2, updated_at_ms = ?3 WHERE id = ?1",
        params![id.to_string(), target_index, now],
    )?;
    tx.commit()?;
    Ok(())
}

/// Same key surface as the `LocalNoteRepository` for callers that want to read
/// the open/closed state of a tree node within a scope.
pub type LocalTreeStateMap = HashMap<String, bool>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{LocalProjectRepository, SqliteLocalProjectRepository};
    use crate::test_support::open_in_memory;

    fn make_pair() -> (
        SqliteLocalProjectRepository,
        SqliteLocalNoteRepository,
        Uuid,
    ) {
        let store = open_in_memory().unwrap();
        let project_repo = SqliteLocalProjectRepository::new(store.clone());
        let note_repo = SqliteLocalNoteRepository::new(store);
        let project = project_repo.create("alpha").unwrap();
        (project_repo, note_repo, project.id)
    }

    #[test]
    fn local_note_repo_create_under_project_returns_uuid_and_depth_zero() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "first").unwrap();
        assert_eq!(note.depth, 0);
        assert!(note.parent_id.is_none());
        assert_eq!(note.sibling_index, 0);
        assert_eq!(note.title, "first");
        assert_eq!(note.project_id, project_id);
    }

    #[test]
    fn local_note_repo_create_under_parent_increments_depth() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let child = n.create(project_id, Some(root.id), "child").unwrap();
        let grand = n.create(project_id, Some(child.id), "grand").unwrap();
        assert_eq!(root.depth, 0);
        assert_eq!(child.depth, 1);
        assert_eq!(grand.depth, 2);
        assert_eq!(child.parent_id, Some(root.id));
        assert_eq!(grand.parent_id, Some(child.id));
        // First child under each parent gets sibling_index 0.
        assert_eq!(child.sibling_index, 0);
        assert_eq!(grand.sibling_index, 0);
    }

    #[test]
    fn local_note_repo_rename_persists() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "original").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        n.rename(note.id, "  Updated  ").unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let got = listed.iter().find(|x| x.id == note.id).unwrap();
        assert_eq!(got.title, "Updated");
        assert!(got.updated_at_ms > note.updated_at_ms);
    }

    #[test]
    fn local_note_repo_rename_rejects_empty_or_whitespace() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "keep").unwrap();
        for bad in ["", "   ", "\t\n  "] {
            let err = n.rename(note.id, bad).unwrap_err();
            assert!(
                matches!(err, StoreError::InvalidArgument(_)),
                "expected InvalidArgument for {bad:?}, got {err:?}"
            );
        }
        let listed = n.list_for_project(project_id).unwrap();
        assert_eq!(listed[0].title, "keep");
    }

    #[test]
    fn local_note_repo_create_with_empty_title_uses_default() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "").unwrap();
        let b = n.create(project_id, None, "   \t\n").unwrap();
        assert_eq!(a.title, DEFAULT_NOTE_TITLE);
        assert_eq!(b.title, DEFAULT_NOTE_TITLE);
    }

    #[test]
    fn local_note_repo_delete_cascades_children() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let _c1 = n.create(project_id, Some(root.id), "c1").unwrap();
        let c2 = n.create(project_id, Some(root.id), "c2").unwrap();
        let _g = n.create(project_id, Some(c2.id), "g").unwrap();

        n.delete(root.id).unwrap();

        let listed = n.list_for_project(project_id).unwrap();
        assert!(
            listed.is_empty(),
            "all descendants should cascade: {listed:?}"
        );
    }

    #[test]
    fn local_note_repo_list_orders_by_sibling_index() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        let c = n.create(project_id, None, "c").unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let roots: Vec<&LocalNote> = listed.iter().filter(|x| x.parent_id.is_none()).collect();
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].id, a.id);
        assert_eq!(roots[1].id, b.id);
        assert_eq!(roots[2].id, c.id);
        assert_eq!(roots[0].sibling_index, 0);
        assert_eq!(roots[1].sibling_index, 1);
        assert_eq!(roots[2].sibling_index, 2);
    }

    #[test]
    fn local_note_repo_create_with_invalid_parent_returns_error() {
        let (_p, n, project_id) = make_pair();
        let phantom = Uuid::new_v4();
        let err = n.create(project_id, Some(phantom), "orphan").unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
    }

    #[test]
    fn local_note_repo_touch_updated_bumps_timestamp() {
        let (_p, n, project_id) = make_pair();
        let note = n.create(project_id, None, "t").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        n.touch_updated(note.id).unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let got = listed.iter().find(|x| x.id == note.id).unwrap();
        assert!(got.updated_at_ms > note.updated_at_ms);
    }

    #[test]
    fn local_note_repo_delete_unknown_id_is_noop() {
        let (_p, n, project_id) = make_pair();
        let _root = n.create(project_id, None, "root").unwrap();
        n.delete(Uuid::new_v4()).unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        assert_eq!(listed.len(), 1);
    }

    fn fetch(n: &SqliteLocalNoteRepository, project_id: Uuid, id: Uuid) -> LocalNote {
        n.list_for_project(project_id)
            .unwrap()
            .into_iter()
            .find(|x| x.id == id)
            .expect("note present")
    }

    #[test]
    fn local_note_repo_move_to_changes_project_and_parent_atomically() {
        let store = open_in_memory().unwrap();
        let project_repo = SqliteLocalProjectRepository::new(store.clone());
        let n = SqliteLocalNoteRepository::new(store);
        let p1 = project_repo.create("p1").unwrap();
        let p2 = project_repo.create("p2").unwrap();

        let a = n.create(p1.id, None, "a").unwrap();
        let _b = n.create(p1.id, None, "b").unwrap();
        let c = n.create(p2.id, None, "c").unwrap();

        // Move `a` from p1 root into p2 under `c` at index 0.
        n.move_to(a.id, p2.id, Some(c.id), 0).unwrap();

        let p1_notes = n.list_for_project(p1.id).unwrap();
        assert_eq!(p1_notes.len(), 1, "only b remains in p1");
        assert_eq!(p1_notes[0].title, "b");
        assert_eq!(p1_notes[0].sibling_index, 0, "b's index should be repacked");

        let p2_notes = n.list_for_project(p2.id).unwrap();
        let moved = p2_notes.iter().find(|x| x.id == a.id).unwrap();
        assert_eq!(moved.project_id, p2.id);
        assert_eq!(moved.parent_id, Some(c.id));
        assert_eq!(moved.sibling_index, 0);
        assert_eq!(moved.depth, 1);
    }

    #[test]
    fn local_note_repo_move_to_rejects_self_descendant_cycle() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let child = n.create(project_id, Some(root.id), "child").unwrap();
        let grand = n.create(project_id, Some(child.id), "grand").unwrap();

        // Self-move.
        let err = n
            .move_to(root.id, project_id, Some(root.id), 0)
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidArgument(_)), "got {err:?}");
        // Move into a descendant.
        let err = n
            .move_to(root.id, project_id, Some(child.id), 0)
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidArgument(_)), "got {err:?}");
        let err = n
            .move_to(root.id, project_id, Some(grand.id), 0)
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidArgument(_)), "got {err:?}");

        // Original tree is untouched.
        let listed = n.list_for_project(project_id).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(fetch(&n, project_id, root.id).depth, 0);
        assert_eq!(fetch(&n, project_id, child.id).depth, 1);
        assert_eq!(fetch(&n, project_id, grand.id).depth, 2);
    }

    #[test]
    fn local_note_repo_move_to_recomputes_depth_for_descendants() {
        let store = open_in_memory().unwrap();
        let project_repo = SqliteLocalProjectRepository::new(store.clone());
        let n = SqliteLocalNoteRepository::new(store);
        let p1 = project_repo.create("p1").unwrap();
        let p2 = project_repo.create("p2").unwrap();

        let a = n.create(p1.id, None, "a").unwrap();
        let b = n.create(p1.id, Some(a.id), "b").unwrap();
        let c = n.create(p1.id, Some(b.id), "c").unwrap();

        let host = n.create(p2.id, None, "host").unwrap();
        let host_child = n.create(p2.id, Some(host.id), "host_child").unwrap();

        // Move a (depth 0 in p1) to be a child of host_child in p2 (which is depth 1).
        n.move_to(a.id, p2.id, Some(host_child.id), 0).unwrap();

        let p2_notes = n.list_for_project(p2.id).unwrap();
        let a_now = p2_notes.iter().find(|x| x.id == a.id).unwrap();
        let b_now = p2_notes.iter().find(|x| x.id == b.id).unwrap();
        let c_now = p2_notes.iter().find(|x| x.id == c.id).unwrap();
        assert_eq!(a_now.depth, 2);
        assert_eq!(b_now.depth, 3);
        assert_eq!(c_now.depth, 4);
        assert_eq!(a_now.project_id, p2.id);
        assert_eq!(b_now.project_id, p2.id);
        assert_eq!(c_now.project_id, p2.id);
        // p1 is now empty.
        let p1_notes = n.list_for_project(p1.id).unwrap();
        assert!(p1_notes.is_empty());
    }

    #[test]
    fn local_note_repo_duplicate_subtree_assigns_new_uuids_and_preserves_shape() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let c1 = n.create(project_id, Some(root.id), "c1").unwrap();
        let _c2 = n.create(project_id, Some(root.id), "c2").unwrap();
        let _g = n.create(project_id, Some(c1.id), "g").unwrap();

        let new_root_id = n.duplicate_subtree(root.id, project_id, None, 999).unwrap();
        assert_ne!(new_root_id, root.id);

        let listed = n.list_for_project(project_id).unwrap();
        // Originals = 4, copies = 4, total = 8.
        assert_eq!(listed.len(), 8);

        let new_root = listed.iter().find(|x| x.id == new_root_id).unwrap();
        assert_eq!(new_root.title, "root");
        assert_eq!(new_root.depth, 0);
        assert!(new_root.parent_id.is_none());

        let new_root_children: Vec<&LocalNote> = listed
            .iter()
            .filter(|x| x.parent_id == Some(new_root_id))
            .collect();
        assert_eq!(new_root_children.len(), 2);
        let titles: Vec<&str> = new_root_children.iter().map(|x| x.title.as_str()).collect();
        assert!(titles.contains(&"c1"));
        assert!(titles.contains(&"c2"));

        // Sibling indexes for the duplicated root should be packed (placed at end).
        let roots: Vec<&LocalNote> = listed.iter().filter(|x| x.parent_id.is_none()).collect();
        assert_eq!(roots.len(), 2);
        let mut indexes: Vec<i64> = roots.iter().map(|x| x.sibling_index).collect();
        indexes.sort();
        assert_eq!(indexes, vec![0, 1]);

        // The copy's grand-child depth must be 2 (one below its copy parent).
        let new_c1 = new_root_children.iter().find(|x| x.title == "c1").unwrap();
        let new_g_list: Vec<&LocalNote> = listed
            .iter()
            .filter(|x| x.parent_id == Some(new_c1.id))
            .collect();
        assert_eq!(new_g_list.len(), 1);
        assert_eq!(new_g_list[0].depth, 2);
        assert_eq!(new_g_list[0].title, "g");
        assert_ne!(new_g_list[0].id, c1.id);
    }

    #[test]
    fn local_note_repo_indent_makes_child_of_previous_sibling_and_increments_depth() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        let g = n.create(project_id, Some(b.id), "g").unwrap();

        n.indent(b.id).unwrap();

        let b_now = fetch(&n, project_id, b.id);
        assert_eq!(b_now.parent_id, Some(a.id));
        assert_eq!(b_now.depth, 1);
        assert_eq!(b_now.sibling_index, 0);
        // Descendant depth follows.
        let g_now = fetch(&n, project_id, g.id);
        assert_eq!(g_now.depth, 2);
        // Roots collapse: only `a`.
        let roots: Vec<LocalNote> = n
            .list_for_project(project_id)
            .unwrap()
            .into_iter()
            .filter(|x| x.parent_id.is_none())
            .collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, a.id);
    }

    #[test]
    fn local_note_repo_indent_at_first_sibling_is_noop() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        n.indent(a.id).unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let a_now = listed.iter().find(|x| x.id == a.id).unwrap();
        assert!(a_now.parent_id.is_none());
        assert_eq!(a_now.sibling_index, 0);
        assert_eq!(a_now.depth, 0);
        let b_now = listed.iter().find(|x| x.id == b.id).unwrap();
        assert!(b_now.parent_id.is_none());
        assert_eq!(b_now.sibling_index, 1);
    }

    #[test]
    fn local_note_repo_outdent_reparents_to_grandparent_and_decrements_depth() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, Some(a.id), "b").unwrap();
        let c = n.create(project_id, Some(b.id), "c").unwrap();
        let d = n.create(project_id, Some(c.id), "d").unwrap();

        n.outdent(c.id).unwrap();

        let c_now = fetch(&n, project_id, c.id);
        assert_eq!(c_now.parent_id, Some(a.id));
        assert_eq!(c_now.depth, 1);
        assert_eq!(c_now.sibling_index, 1, "placed after old parent b");
        // Descendant follows (d).
        let d_now = fetch(&n, project_id, d.id);
        assert_eq!(d_now.depth, 2);
        assert_eq!(d_now.parent_id, Some(c.id));
    }

    #[test]
    fn local_note_repo_outdent_at_depth_zero_is_noop() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        n.outdent(a.id).unwrap();
        let a_now = fetch(&n, project_id, a.id);
        assert!(a_now.parent_id.is_none());
        assert_eq!(a_now.depth, 0);
        assert_eq!(a_now.sibling_index, 0);
    }

    #[test]
    fn local_note_repo_move_up_swaps_with_previous_sibling() {
        let (_p, n, project_id) = make_pair();
        let a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        let c = n.create(project_id, None, "c").unwrap();

        n.move_up(c.id).unwrap();
        let listed = n.list_for_project(project_id).unwrap();
        let order: Vec<(Uuid, i64)> = listed
            .iter()
            .filter(|x| x.parent_id.is_none())
            .map(|x| (x.id, x.sibling_index))
            .collect();
        let by_index: Vec<Uuid> = {
            let mut v = order.clone();
            v.sort_by_key(|x| x.1);
            v.into_iter().map(|x| x.0).collect()
        };
        assert_eq!(by_index, vec![a.id, c.id, b.id]);

        // Move-up at the first slot is a no-op.
        n.move_up(a.id).unwrap();
        let listed2 = n.list_for_project(project_id).unwrap();
        let a_now = listed2.iter().find(|x| x.id == a.id).unwrap();
        assert_eq!(a_now.sibling_index, 0);
    }

    #[test]
    fn local_note_repo_move_down_at_last_sibling_is_noop() {
        let (_p, n, project_id) = make_pair();
        let _a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        n.move_down(b.id).unwrap();
        let b_now = fetch(&n, project_id, b.id);
        assert_eq!(b_now.sibling_index, 1);
    }

    // ===== Plans-Phase-8-explorer-undo: snapshot / restore round-trip =====

    #[test]
    fn snapshot_subtree_returns_root_and_descendants_in_bfs_order() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let c1 = n.create(project_id, Some(root.id), "c1").unwrap();
        let c2 = n.create(project_id, Some(root.id), "c2").unwrap();
        let g = n.create(project_id, Some(c2.id), "g").unwrap();

        let snap = n.snapshot_subtree(root.id).unwrap();
        assert_eq!(snap.root_id, root.id);
        // Root comes first, then children of root, then grandchildren.
        assert_eq!(snap.notes[0].id, root.id);
        let ids: Vec<Uuid> = snap.notes.iter().map(|n| n.id).collect();
        assert!(ids.contains(&c1.id));
        assert!(ids.contains(&c2.id));
        assert!(ids.contains(&g.id));
        assert_eq!(snap.notes.len(), 4);
    }

    #[test]
    fn snapshot_subtree_then_delete_then_restore_reproduces_tree() {
        let (_p, n, project_id) = make_pair();
        let root = n.create(project_id, None, "root").unwrap();
        let c1 = n.create(project_id, Some(root.id), "c1").unwrap();
        let _c2 = n.create(project_id, Some(root.id), "c2").unwrap();
        let _g = n.create(project_id, Some(c1.id), "g").unwrap();

        let before: Vec<(Uuid, Option<Uuid>, i64, String)> = n
            .list_for_project(project_id)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.parent_id, r.sibling_index, r.title))
            .collect();

        let snap = n.snapshot_subtree(root.id).unwrap();
        n.delete(root.id).unwrap();
        assert!(n.list_for_project(project_id).unwrap().is_empty());

        n.restore_subtree(&snap).unwrap();
        let after: Vec<(Uuid, Option<Uuid>, i64, String)> = n
            .list_for_project(project_id)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.parent_id, r.sibling_index, r.title))
            .collect();
        assert_eq!(before, after, "restore must reproduce the pre-delete tree");
    }

    #[test]
    fn snapshot_subtree_missing_root_returns_not_found() {
        let (_p, n, _project_id) = make_pair();
        let bogus = Uuid::new_v4();
        match n.snapshot_subtree(bogus) {
            Err(StoreError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn restore_subtree_densifies_destination_siblings() {
        // a, b, c at root. Snapshot b, delete it, then restore — siblings
        // should bump back up to make room at b's original sibling_index.
        let (_p, n, project_id) = make_pair();
        let _a = n.create(project_id, None, "a").unwrap();
        let b = n.create(project_id, None, "b").unwrap();
        let c = n.create(project_id, None, "c").unwrap();

        let snap = n.snapshot_subtree(b.id).unwrap();
        n.delete(b.id).unwrap();
        // After delete c stays at sibling_index=2 (no auto-densify on
        // delete). Restore should still slot b back at index=1 and shift
        // any siblings at >=1 up by 1 first.
        n.restore_subtree(&snap).unwrap();

        let listed = n.list_for_project(project_id).unwrap();
        let by_id: std::collections::HashMap<Uuid, &LocalNote> =
            listed.iter().map(|r| (r.id, r)).collect();
        assert_eq!(by_id.get(&b.id).unwrap().sibling_index, 1);
        assert_eq!(by_id.get(&c.id).unwrap().sibling_index, 3);
    }

    #[test]
    fn note_kind_string_round_trip_covers_every_variant() {
        for &kind in NoteKind::all_creatable() {
            let s = kind.as_str();
            assert_eq!(NoteKind::from_str(s), kind, "round-trip failed for {s}");
        }
        // Unknown strings collapse to Markdown so old rows stay readable.
        assert_eq!(NoteKind::from_str("definitely-not-a-kind"), NoteKind::Markdown);
    }

    #[test]
    fn note_kind_format_id_matches_as_str() {
        for &kind in NoteKind::all_creatable() {
            assert_eq!(kind.format_id(), kind.as_str());
        }
    }

    #[test]
    fn create_with_kind_persists_each_variant() {
        let (_p, n, project_id) = make_pair();
        for &kind in NoteKind::all_creatable() {
            let label = kind.as_str();
            let created = n
                .create_with_kind(project_id, None, label, kind)
                .unwrap_or_else(|e| panic!("create_with_kind({label}) failed: {e}"));
            assert_eq!(created.kind, kind, "kind mismatch for {label}");
        }
        let listed = n.list_for_project(project_id).unwrap();
        for &kind in NoteKind::all_creatable() {
            let label = kind.as_str();
            let row = listed.iter().find(|r| r.title == label).unwrap();
            assert_eq!(row.kind, kind);
        }
    }
}
