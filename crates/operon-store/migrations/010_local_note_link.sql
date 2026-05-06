-- Plans-Phase-5-vfs-wikilinks: link graph for `[[…]]` / `![[…]]` references.
--
-- Each row is one occurrence in `source_note_id`'s body. `target_text` is
-- the raw inner string (so `[[Project/Note]]` stores `Project/Note`).
-- `target_note_id` is resolved at write time when possible; `ON DELETE
-- SET NULL` lets the renderer detect broken links cheaply.
--
-- The `(source_note_id, target_text)` PK lets the save-time rebuild use a
-- straightforward delete-then-insert pattern without needing a SELECT to
-- diff the previous state.

CREATE TABLE local_note_link (
    source_note_id TEXT NOT NULL REFERENCES local_note(id) ON DELETE CASCADE,
    target_text    TEXT NOT NULL,
    target_note_id TEXT REFERENCES local_note(id) ON DELETE SET NULL,
    is_embed       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source_note_id, target_text)
);
CREATE INDEX idx_local_note_link_target ON local_note_link(target_note_id);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (10, 0);
