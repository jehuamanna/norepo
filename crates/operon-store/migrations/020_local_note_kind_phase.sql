-- Phase B of three-tier SDLC restructure: extend the
-- `local_note.kind` CHECK constraint to also allow 'phase'. A phase
-- note is a top-level container at the project root that groups a
-- batch of requirements + epics together (Discovery, Phase 1,
-- Multiplayer MVP, …). Ordering, label, and other phase metadata
-- live in the note's YAML frontmatter (`phase_order`, `phase_label`,
-- …) so no new columns are needed here — only the CHECK widens.
--
-- Same SQLite-doesn't-DROP-CONSTRAINT pattern as migrations 011 /
-- 015 / 016: rebuild the table, copy the data, swap names. Deferred
-- FKs keep mid-rebuild reference checks from firing.

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
                  CHECK (kind IN ('markdown', 'mdx', 'image', 'canvas',
                                  'excalidraw', 'kanban', 'code',
                                  'skill', 'workflow', 'artifact',
                                  'phase')),
    blob_path     TEXT,
    slug          TEXT
);

INSERT INTO local_note_new (
    id, project_id, parent_id, sibling_index, depth, title,
    created_at_ms, updated_at_ms, kind, blob_path, slug
)
SELECT
    id, project_id, parent_id, sibling_index, depth, title,
    created_at_ms, updated_at_ms, kind, blob_path, slug
FROM local_note;

DROP TABLE local_note;
ALTER TABLE local_note_new RENAME TO local_note;

CREATE INDEX idx_local_note_project_sibling
    ON local_note (project_id, parent_id, sibling_index);

-- Re-create the slug uniqueness index that migration 018 added; the
-- DROP TABLE above wiped it along with the original table.
CREATE UNIQUE INDEX idx_local_note_slug_per_parent
    ON local_note (project_id, COALESCE(parent_id, ''), slug)
    WHERE slug IS NOT NULL;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (20, 0);
