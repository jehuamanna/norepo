-- M1-companion-claude-code: bind a local project to a target git repository.
-- The companion-pane Claude Code subprocess uses this path as `cwd` so the
-- assistant operates against the right codebase. NULL means "not set yet" —
-- the chat UI is disabled until the user picks a folder via the project's
-- "Set Repository…" context menu.

ALTER TABLE local_project ADD COLUMN repo_path TEXT NULL;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (12, 0);
