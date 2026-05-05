CREATE TABLE IF NOT EXISTS messages (
    id BLOB PRIMARY KEY,
    scope_kind INTEGER NOT NULL,            -- 0=User 1=Project 2=Team
    scope_id BLOB,                          -- nullable for User scope
    session BLOB NOT NULL,
    role TEXT NOT NULL,
    content_json TEXT NOT NULL,             -- serialized Vec<ContentBlock>
    metadata_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS messages_scope_session ON messages (scope_kind, scope_id, session);
CREATE INDEX IF NOT EXISTS messages_created_at ON messages (created_at DESC);
-- Vector column placeholder: leave for sqlite-vec integration in a later seed
