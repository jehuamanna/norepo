-- M1.5a-multi-session: persist companion-pane chat sessions.
--
-- Sessions are scoped either to a `local_project` (cwd = project's
-- repo_path) or to the vault as a whole (`scope_kind = 'vault'`,
-- `scope_id IS NULL`). The companion's left rail lists rows from this
-- table filtered by the active scope. Transcript message bodies live in
-- operon-core's existing `messages` table — keyed on the same session
-- UUID — so we don't duplicate them here.

CREATE TABLE chat_session (
    id                  TEXT PRIMARY KEY,
    scope_kind          TEXT NOT NULL CHECK (scope_kind IN ('project', 'vault')),
    scope_id            TEXT NULL,
    label               TEXT NOT NULL,
    claude_session_id   TEXT NULL,
    last_used_ms        INTEGER NOT NULL,
    created_ms          INTEGER NOT NULL,
    -- Vault rows must have NULL scope_id; project rows must have a value.
    CHECK ((scope_kind = 'vault'   AND scope_id IS NULL)
        OR (scope_kind = 'project' AND scope_id IS NOT NULL))
);

CREATE INDEX idx_chat_session_scope
    ON chat_session (scope_kind, scope_id, last_used_ms DESC);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (13, 0);
