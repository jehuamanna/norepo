ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN last_login_at_ms INTEGER;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (2, 0);
