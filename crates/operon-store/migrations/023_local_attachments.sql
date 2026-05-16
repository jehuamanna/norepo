-- Local-mode attachments table. Mirrors the cloud `attachments` schema
-- but FKs to `local_note(id)` so chat-mode "attach this screenshot to
-- my Features note" works inside an Operon vault. The cloud
-- `attachments` table is left untouched for cloud-mode workflows.
--
-- `(note_id, sha256_hex)` is UNIQUE so attaching the same blob twice
-- to the same note dedupes naturally; cross-note duplicates remain
-- separate rows because the dedup story across notes belongs to the
-- content-addressed blob store under `<vault>/.operon/images/`.
--
-- ON DELETE CASCADE: deleting the host note via
-- `LocalNoteRepository::delete` removes the attachment rows
-- automatically. The on-disk blobs are NOT cleaned up here —
-- content-addressed, may be shared with image notes or other
-- attachments; refcount-based GC is a separate concern.

-- IF NOT EXISTS so dev DBs that already carry `local_attachments` from
-- an earlier numbering of this same migration (when it briefly lived as
-- version 22 before being renumbered) come up cleanly instead of
-- panicking on `Store::open`. The column shape has not changed across
-- the rename, so reusing the existing table is safe.
CREATE TABLE IF NOT EXISTS local_attachments (
    id            TEXT PRIMARY KEY,
    note_id       TEXT NOT NULL REFERENCES local_note(id) ON DELETE CASCADE,
    filename      TEXT NOT NULL,
    mime_type     TEXT,
    sha256_hex    TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL,
    blob_path     TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (note_id, sha256_hex)
);

CREATE INDEX IF NOT EXISTS idx_local_attachments_note ON local_attachments(note_id);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (23, 0);
