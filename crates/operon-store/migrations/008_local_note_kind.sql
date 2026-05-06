-- Plans-Phase-6-image-notes: add a `kind` column to `local_note` so the
-- existing tree can carry both Markdown and Image notes. SQLite supports
-- `ADD COLUMN` with a NOT NULL default, so this migration is non-destructive
-- on existing databases — every prior row gets `'markdown'`.

ALTER TABLE local_note ADD COLUMN kind TEXT NOT NULL DEFAULT 'markdown'
    CHECK (kind IN ('markdown', 'image'));

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (8, 0);
