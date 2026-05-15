-- Three-tier SDLC, CE-as-first-class follow-up to migration 020:
-- extend the `local_note.kind` CHECK constraint to also allow 'ce'.
-- A CE (Customer Engineering) note is a project-root singleton
-- that holds raw customer materials (Markdown, sketches, nested
-- requirements). It used to be modelled as an `Artifact` with
-- `artifact_kind: requirement` in frontmatter; promoting it to a
-- first-class kind makes discovery a SQL query rather than a body
-- parse. Companion code at `src/plugins/phase/ce_migration.rs` does
-- the one-shot flip of existing legacy CE rows on project open.
--
-- Same SQLite-doesn't-DROP-CONSTRAINT pattern as migrations 011 /
-- 015 / 016 / 020: rebuild the table, copy the data, swap names.
-- Deferred FKs keep mid-rebuild reference checks from firing.

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
                                  'phase', 'ce')),
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

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (21, 0);
