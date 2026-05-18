# Database Schema

## Overview

Operon uses **SQLite** as its embedded database with 16 automatic migrations managed by `operon-store`. The schema supports both Cloud Mode (RBAG) and Local Mode tables.

---

## Entity Relationship Diagram

```mermaid
erDiagram
    users ||--o{ sessions : "has"
    users ||--o{ memberships : "belongs to"
    orgs ||--o{ departments : "has"
    orgs ||--o{ teams : "has"
    orgs ||--o{ projects : "has"
    orgs ||--o{ memberships : "has"
    teams ||--o{ team_members : "has"
    teams ||--o{ team_projects : "accesses"
    projects ||--o{ notes : "contains"
    projects ||--o{ team_projects : "assigned to"
    notes ||--o{ note_updates : "versioned by"
    notes ||--o{ attachments : "has"
    users ||--o{ audit_log : "generates"

    users {
        TEXT id PK
        TEXT email UK
        TEXT name
        TEXT password_hash
        BOOLEAN must_change_password
        INTEGER created_at
        INTEGER updated_at
    }

    sessions {
        TEXT token PK
        TEXT user_id FK
        INTEGER created_at
        INTEGER expires_at
    }

    orgs {
        TEXT id PK
        TEXT name
        INTEGER created_at
    }

    departments {
        TEXT id PK
        TEXT org_id FK
        TEXT name
        INTEGER created_at
    }

    teams {
        TEXT id PK
        TEXT org_id FK
        TEXT name
        INTEGER created_at
    }

    team_members {
        TEXT team_id FK
        TEXT user_id FK
        TEXT role
    }

    projects {
        TEXT id PK
        TEXT org_id FK
        TEXT name
        TEXT description
        INTEGER created_at
    }

    team_projects {
        TEXT team_id FK
        TEXT project_id FK
    }

    notes {
        TEXT id PK
        TEXT project_id FK
        TEXT title
        TEXT kind
        BLOB body
        INTEGER created_at
        INTEGER updated_at
    }

    note_updates {
        TEXT id PK
        TEXT note_id FK
        BLOB delta
        INTEGER created_at
    }

    attachments {
        TEXT id PK
        TEXT note_id FK
        TEXT filename
        TEXT content_type
        BLOB data
        INTEGER created_at
    }

    audit_log {
        TEXT id PK
        TEXT user_id FK
        TEXT action
        TEXT resource_type
        TEXT resource_id
        TEXT details
        INTEGER created_at
    }

    memberships {
        TEXT user_id FK
        TEXT org_id FK
        TEXT role
        INTEGER created_at
    }
}
```

---

## Cloud Mode Tables

### `users`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `email` | TEXT | UNIQUE, NOT NULL | User email |
| `name` | TEXT | NOT NULL | Display name |
| `password_hash` | TEXT | NOT NULL | Argon2 hash |
| `must_change_password` | BOOLEAN | DEFAULT false | Force password change flag |
| `created_at` | INTEGER | NOT NULL | Unix timestamp (ms) |
| `updated_at` | INTEGER | NOT NULL | Unix timestamp (ms) |

### `sessions`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `token` | TEXT | PRIMARY KEY | Session token (SHA-256) |
| `user_id` | TEXT | FK → users | Owner |
| `created_at` | INTEGER | NOT NULL | Creation time |
| `expires_at` | INTEGER | NOT NULL | Expiration time |

### `orgs`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `name` | TEXT | NOT NULL | Organization name |
| `created_at` | INTEGER | NOT NULL | Creation time |

### `departments`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `org_id` | TEXT | FK → orgs | Parent organization |
| `name` | TEXT | NOT NULL | Department name |
| `created_at` | INTEGER | NOT NULL | Creation time |

### `teams`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `org_id` | TEXT | FK → orgs | Parent organization |
| `name` | TEXT | NOT NULL | Team name |
| `created_at` | INTEGER | NOT NULL | Creation time |

### `team_members`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `team_id` | TEXT | FK → teams | Team |
| `user_id` | TEXT | FK → users | Member |
| `role` | TEXT | NOT NULL | Role within team |

### `projects`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `org_id` | TEXT | FK → orgs | Parent organization |
| `name` | TEXT | NOT NULL | Project name |
| `description` | TEXT | | Optional description |
| `created_at` | INTEGER | NOT NULL | Creation time |

### `team_projects`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `team_id` | TEXT | FK → teams | Team |
| `project_id` | TEXT | FK → projects | Project |

### `notes`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `project_id` | TEXT | FK → projects | Parent project |
| `title` | TEXT | NOT NULL | Note title |
| `kind` | TEXT | NOT NULL | Note kind (markdown, code, richtext, etc.) |
| `body` | BLOB | | Raw content bytes |
| `created_at` | INTEGER | NOT NULL | Creation time |
| `updated_at` | INTEGER | NOT NULL | Last modified |

### `note_updates`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `note_id` | TEXT | FK → notes | Parent note |
| `delta` | BLOB | NOT NULL | Loro CRDT delta |
| `created_at` | INTEGER | NOT NULL | Creation time |

### `attachments`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `note_id` | TEXT | FK → notes | Parent note |
| `filename` | TEXT | NOT NULL | Original filename |
| `content_type` | TEXT | NOT NULL | MIME type |
| `data` | BLOB | NOT NULL | File content |
| `created_at` | INTEGER | NOT NULL | Creation time |

### `audit_log`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `user_id` | TEXT | FK → users | Actor |
| `action` | TEXT | NOT NULL | Action performed |
| `resource_type` | TEXT | NOT NULL | Resource type |
| `resource_id` | TEXT | | Affected resource ID |
| `details` | TEXT | | JSON details |
| `created_at` | INTEGER | NOT NULL | Timestamp |

### `memberships`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `user_id` | TEXT | FK → users | Member |
| `org_id` | TEXT | FK → orgs | Organization |
| `role` | TEXT | NOT NULL | Organization role |
| `created_at` | INTEGER | NOT NULL | Join time |

### `chat_sessions`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `title` | TEXT | | Session title |
| `created_at` | INTEGER | NOT NULL | Creation time |
| `updated_at` | INTEGER | NOT NULL | Last activity |

### `chat_messages`

| Column | Type | Constraints | Description |
|---|---|---|---|
| `id` | TEXT | PRIMARY KEY | UUID |
| `session_id` | TEXT | FK → chat_sessions | Parent session |
| `role` | TEXT | NOT NULL | user/assistant/system |
| `content` | TEXT | NOT NULL | Message content |
| `created_at` | INTEGER | NOT NULL | Send time |

---

## Local Mode Tables

### `local_user`

| Column | Type | Description |
|---|---|---|
| `id` | TEXT PK | UUID |
| `name` | TEXT | Local user display name |

### `local_app_settings`

| Column | Type | Description |
|---|---|---|
| `key` | TEXT PK | Setting key (e.g., `vault.root.path`) |
| `value` | TEXT | Setting value (JSON) |

### `local_project`

| Column | Type | Description |
|---|---|---|
| `id` | TEXT PK | UUID |
| `name` | TEXT | Project name |
| `repo_path` | TEXT | Bound Git repository path |
| `created_at` | INTEGER | Creation time |

### `local_note`

| Column | Type | Description |
|---|---|---|
| `id` | TEXT PK | UUID |
| `project_id` | TEXT FK | Parent project |
| `title` | TEXT | Note title |
| `kind` | TEXT | Note kind (markdown/code/richtext/skill/workflow/artifact) |
| `blob_path` | TEXT | Path to image blob (if image note) |
| `created_at` | INTEGER | Creation time |
| `updated_at` | INTEGER | Last modified |

### `local_note_link`

| Column | Type | Description |
|---|---|---|
| `source_id` | TEXT FK | Source note |
| `target_id` | TEXT FK | Target note (wikilink destination) |
| `link_text` | TEXT | Display text |

### `local_tree_state`

| Column | Type | Description |
|---|---|---|
| `node_id` | TEXT PK | Tree node ID |
| `collapsed` | BOOLEAN | Collapse state |

### `local_search`

FTS5 virtual table for full-text search across local notes.

---

## Migration History

| # | Name | Description |
|---|---|---|
| 001 | `initial` | Core schema (users, orgs, projects, notes, etc.) |
| 002 | `users_password_flags` | Added `must_change_password` to users |
| 003 | `audit_log` | Audit trail table |
| 004 | `note_updates` | Note versioning with CRDT deltas |
| 005 | `local_mode` | Local user + app settings tables |
| 006 | `local_projects` | Local project table |
| 007 | `local_notes` | Local note table |
| 008 | `local_note_kind` | Note kind field (markdown/code/tiptap) |
| 009 | `local_note_blob_path` | Image blob path column |
| 010 | `local_note_link` | Wikilink tracking table |
| 011 | `local_note_kind_extend` | Extended note kinds |
| 012 | `local_project_repo_path` | Bound repo path column |
| 013 | `chat_sessions` | Chat session tracking |
| 014 | `chat_messages` | Chat message history |
| 015 | `local_note_kind_skill_workflow` | Skill + Workflow note kinds |
| 016 | `local_note_kind_artifact` | Artifact kinds (requirements, epic, feature, etc.) |

Migrations run automatically on database open. No manual migration commands needed.

---

## Repository Layer

Data access is abstracted via repository traits in `operon-store`:

| Repository | Table(s) | Operations |
|---|---|---|
| `UserRepo` | `users` | CRUD, find by email |
| `OrgRepo` | `orgs` | CRUD |
| `TeamRepo` | `teams` | CRUD, list by org |
| `TeamMemberRepo` | `team_members` | Add/remove members |
| `TeamProjectRepo` | `team_projects` | Assign/revoke access |
| `ProjectRepo` | `projects` | CRUD, list by org |
| `DepartmentRepo` | `departments` | CRUD, list by org |
| `MembershipRepo` | `memberships` | CRUD |
| `NoteRepo` | `notes` | CRUD, list by project |
| `NoteUpdateRepo` | `note_updates` | Create, list by note |
| `AttachmentRepo` | `attachments` | CRUD |
| `ChatSessionRepo` | `chat_sessions` | CRUD |
| `ChatMessageRepo` | `chat_messages` | CRUD, list by session |
| `SessionRepo` | `sessions` | Create, validate, delete |
| `InviteRepo` | (invites) | Create, consume |
| `AuditRepo` | `audit_log` | Create, query |
| `LocalUserRepo` | `local_user` | CRUD |
| `LocalProjectRepo` | `local_project` | CRUD |
| `LocalNoteRepo` | `local_note` | CRUD, search |
| `LocalTreeStateRepo` | `local_tree_state` | Get/set collapse state |
| `LocalSearchRepo` | `local_search` | FTS query |
| `LocalSettingsRepo` | `local_app_settings` | Get/set settings |
| `LocalNoteLinkRepo` | `local_note_link` | Create/query wikilinks |
