-- Initial schema. All UUIDs stored as TEXT; timestamps as INTEGER ms since epoch.

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT UNIQUE NOT NULL COLLATE NOCASE,
    display_name TEXT,
    password_hash TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE orgs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    flavour TEXT NOT NULL CHECK (flavour IN ('local','non_local','system')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE departments (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (org_id, name)
);

CREATE TABLE teams (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (org_id, name)
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (org_id, name)
);

CREATE TABLE notes (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    parent_id TEXT REFERENCES notes(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body_markdown TEXT,
    loro_snapshot BLOB,
    sibling_index INTEGER NOT NULL DEFAULT 0,
    type TEXT NOT NULL DEFAULT 'markdown',
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE memberships (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('master_admin','org_admin','user')),
    department_id TEXT REFERENCES departments(id) ON DELETE SET NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (user_id, org_id),
    CHECK (role = 'master_admin' OR department_id IS NOT NULL)
);

CREATE TABLE team_members (
    id TEXT PRIMARY KEY,
    membership_id TEXT NOT NULL REFERENCES memberships(id) ON DELETE CASCADE,
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (membership_id, team_id)
);

CREATE TABLE team_projects (
    id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (team_id, project_id)
);

CREATE TABLE invites (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL COLLATE NOCASE,
    org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('master_admin','org_admin','user')),
    department_id TEXT REFERENCES departments(id) ON DELETE SET NULL,
    invited_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT UNIQUE NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    accepted_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    active_org_id TEXT REFERENCES orgs(id) ON DELETE SET NULL,
    token_hash TEXT UNIQUE NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL
);

CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    mime_type TEXT,
    sha256_hex TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    blob_path TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (note_id, sha256_hex)
);

CREATE INDEX idx_notes_project_parent ON notes(project_id, parent_id, sibling_index);
CREATE INDEX idx_memberships_user ON memberships(user_id);
CREATE INDEX idx_memberships_org ON memberships(org_id);
CREATE INDEX idx_team_members_membership ON team_members(membership_id);
CREATE INDEX idx_team_projects_team ON team_projects(team_id);
CREATE INDEX idx_team_projects_project ON team_projects(project_id);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_invites_email_unaccepted ON invites(email) WHERE accepted_at_ms IS NULL;

INSERT INTO orgs (id, name, flavour, created_at_ms, updated_at_ms)
VALUES ('00000000-0000-0000-0000-000000000000', 'system', 'system', 0, 0);

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (1, 0);
