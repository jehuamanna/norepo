CREATE TABLE local_note (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    parent_id     TEXT REFERENCES local_note(id) ON DELETE CASCADE,
    sibling_index INTEGER NOT NULL,
    depth         INTEGER NOT NULL DEFAULT 0,
    title         TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_local_note_project_sibling
    ON local_note (project_id, parent_id, sibling_index);

CREATE TABLE local_tree_state (
    scope     TEXT NOT NULL,
    node_id   TEXT NOT NULL,
    is_open   INTEGER NOT NULL,
    PRIMARY KEY (scope, node_id)
);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (7, 0);
