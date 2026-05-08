-- M1.5b-task-12: persist companion-pane transcripts so reopening a chat
-- restores its history. One row per visible transcript block (user line,
-- assistant text, thinking, tool call, system notice). Tool calls are
-- joined to their later results by `tool_use_id` so a single row holds
-- the round-trip.
--
-- The schema is deliberately flat (variant kind + JSON body) so adding a
-- new TranscriptItem variant later (vision attachments, plan-mode marker,
-- permission prompts) doesn't need another migration.

CREATE TABLE chat_message (
    id                  TEXT PRIMARY KEY,
    chat_session_id     TEXT NOT NULL REFERENCES chat_session(id) ON DELETE CASCADE,
    sequence            INTEGER NOT NULL,
    kind                TEXT NOT NULL CHECK (kind IN
                            ('user', 'assistant', 'thinking', 'tool_call', 'system')),
    -- Set only for kind='tool_call'. Lets us locate the row when the
    -- matching tool_result event arrives later in the stream.
    tool_use_id         TEXT NULL,
    body_json           TEXT NOT NULL,
    created_at_ms       INTEGER NOT NULL
);

CREATE INDEX idx_chat_message_session_sequence
    ON chat_message (chat_session_id, sequence);

-- Partial index: only tool_call rows participate in tool_use_id lookups,
-- and they're a small fraction of total messages.
CREATE INDEX idx_chat_message_tool_use
    ON chat_message (chat_session_id, tool_use_id)
    WHERE tool_use_id IS NOT NULL;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (14, 0);
