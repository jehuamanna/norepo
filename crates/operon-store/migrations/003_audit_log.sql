CREATE TABLE audit_log (
    id TEXT PRIMARY KEY,
    user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    org_id TEXT REFERENCES orgs(id) ON DELETE SET NULL,
    role TEXT,
    action TEXT NOT NULL,
    scope_type TEXT NOT NULL,
    scope_id TEXT,
    outcome TEXT NOT NULL CHECK (outcome IN ('allowed','denied','error')),
    payload_json TEXT,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_audit_user_time ON audit_log(user_id, created_at_ms);
CREATE INDEX idx_audit_org_time ON audit_log(org_id, created_at_ms);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (3, 0);
