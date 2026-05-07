-- Operon-Phase-1-note-kinds: extend the `local_note.kind` CHECK constraint
-- to allow the full set of planned note kinds. Migration 008 only allowed
-- ('markdown', 'image'); this migration broadens it to also include
-- mdx, canvas, excalidraw, kanban, and code so the explorer's + dropdown
-- can create any of them.
--
-- SQLite cannot `ALTER TABLE ... DROP CONSTRAINT`, so we rebuild the table
-- with the new CHECK in place. `PRAGMA defer_foreign_keys = ON` lets the
-- rebuild happen inside the migration transaction without the FK from
-- `local_note_link.source_note_id` / `target_note_id` firing on the
-- intermediate DROP. The PRAGMA auto-clears at COMMIT.

PRAGMA defer_foreign_keys = ON;

CREATE TABLE local_note_new (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    parent_id     TEXT REFERENCES local_note(id) ON DELETE CASCADE,
    sibling_index INTEGER NOT NULL,
    depth         INTEGER NOT NULL DEFAULT 0,
    title         TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    kind          TEXT NOT NULL DEFAULT 'markdown'
                  CHECK (kind IN ('markdown', 'mdx', 'image', 'canvas', 'excalidraw', 'kanban', 'code')),
    blob_path     TEXT
);

INSERT INTO local_note_new (
    id, project_id, parent_id, sibling_index, depth, title,
    created_at_ms, updated_at_ms, kind, blob_path
)
SELECT
    id, project_id, parent_id, sibling_index, depth, title,
    created_at_ms, updated_at_ms, kind, blob_path
FROM local_note;

DROP TABLE local_note;
ALTER TABLE local_note_new RENAME TO local_note;

CREATE INDEX idx_local_note_project_sibling
    ON local_note (project_id, parent_id, sibling_index);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (11, 0);
