-- SDLC pipeline (BA / Architect / Engineer phases): extend the
-- `local_note.kind` CHECK constraint to also allow 'artifact'. An
-- artifact is a note produced by a workflow skill run that the user
-- wants to track through a status lifecycle (pending → approved →
-- dirty etc.); the artifact-specific metadata lives in the note's
-- YAML frontmatter, so no new columns here — only the CHECK widens.
--
-- Same SQLite-doesn't-DROP-CONSTRAINT pattern as migrations 011 + 015:
-- rebuild the table, copy the data, swap names. Deferred FKs keep
-- mid-rebuild reference checks from firing.

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
                                  'skill', 'workflow', 'artifact')),
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

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (16, 0);
