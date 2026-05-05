CREATE TABLE note_updates (
    id TEXT PRIMARY KEY,
    note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    update_blob BLOB NOT NULL,
    applied_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_note_updates_note_time ON note_updates(note_id, applied_at_ms);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (4, 0);
