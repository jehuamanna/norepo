-- Artifact-on-disk 1:1: every artifact note maps to `.operon/artifacts/<slug>/.../index.md`.
-- The slug is captured at create/rename time so the canonical disk path is stable
-- across title edits that would otherwise re-slugify into a colliding sibling.
--
-- Non-artifact rows keep `slug = NULL`. The unique index is partial so only
-- artifact rows participate in collision checks, scoped per (project, parent).

ALTER TABLE local_note ADD COLUMN slug TEXT;

CREATE UNIQUE INDEX idx_local_note_slug_per_parent
    ON local_note (project_id, COALESCE(parent_id, ''), slug)
    WHERE slug IS NOT NULL;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (18, 0);
