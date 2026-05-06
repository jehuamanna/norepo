-- Plans-Phase-6-image-notes: image notes carry a vault-relative blob path
-- (e.g. `.operon/images/<sha>.png`). Markdown notes leave this NULL. Path
-- is computed by `images::write_image` then stored here so the renderer
-- can `read_image(vault, blob_path)` back to bytes when the image-tab view
-- mounts.
--
-- An equivalent local_attachment table was considered but ruled out as
-- over-engineering for the v1 image-note shape: each image note has
-- exactly one blob, the blob lives content-addressed (so multiple notes
-- can share a sha by accident), and refcount-based GC just iterates
-- local_note WHERE blob_path = ?.

ALTER TABLE local_note ADD COLUMN blob_path TEXT;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (9, 0);
