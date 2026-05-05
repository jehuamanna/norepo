CREATE TABLE local_project (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    sibling_index INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_local_project_sibling ON local_project (sibling_index);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (6, 0);
