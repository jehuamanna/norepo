# API Reference

## Overview

The Operon API server (`operon-api-server`) provides an Axum-based REST API for **Cloud Mode (RBAG)**. It handles multi-user collaboration with RBAC, note CRUD, organization management, and export/import.

**Default**: `http://127.0.0.1:7878`

**Authentication**: Bearer token in `Authorization` header (obtained via login endpoint).

---

## Authentication

### POST `/api/auth/login`

Login with email and password.

**Request**:
```json
{
  "email": "user@example.com",
  "password": "secret"
}
```

**Response** (200):
```json
{
  "token": "session-token-string",
  "user": {
    "id": "uuid",
    "email": "user@example.com",
    "name": "User Name"
  }
}
```

**Errors**:
- `401` — Invalid credentials
- `422` — Validation error

---

### POST `/api/auth/signup`

Create a new account (if self-registration is enabled).

**Request**:
```json
{
  "email": "user@example.com",
  "password": "secret",
  "name": "User Name"
}
```

**Response** (201): Same as login response.

---

### POST `/api/auth/password-reset`

Reset password (requires valid session or invite token).

**Request**:
```json
{
  "old_password": "old-secret",
  "new_password": "new-secret"
}
```

**Response** (200): `{ "ok": true }`

---

## Session

### GET `/api/session`

Validate current session token.

**Headers**: `Authorization: Bearer <token>`

**Response** (200):
```json
{
  "valid": true,
  "user_id": "uuid"
}
```

**Errors**:
- `401` — Invalid/expired token

---

## User Profile

### GET `/api/me`

Get current authenticated user profile.

**Headers**: `Authorization: Bearer <token>`

**Response** (200):
```json
{
  "id": "uuid",
  "email": "user@example.com",
  "name": "User Name",
  "orgs": [...],
  "role": "member"
}
```

---

## Organizations

### GET `/api/orgs`

List organizations the user belongs to.

### POST `/api/orgs`

Create a new organization.

**Request**:
```json
{
  "name": "My Organization"
}
```

---

## Departments

### GET `/api/orgs/:org_id/departments`

List departments in an organization.

### POST `/api/orgs/:org_id/departments`

Create a department.

**Request**:
```json
{
  "name": "Engineering"
}
```

---

## Teams

### GET `/api/orgs/:org_id/teams`

List teams in an organization.

### POST `/api/orgs/:org_id/teams`

Create a team.

### Team Members

#### GET `/api/teams/:team_id/members`

List team members.

#### POST `/api/teams/:team_id/members`

Add member to team.

### Team Projects

#### GET `/api/teams/:team_id/projects`

List projects accessible by team.

#### POST `/api/teams/:team_id/projects`

Assign project to team.

---

## Projects

### GET `/api/orgs/:org_id/projects`

List projects in an organization.

### POST `/api/orgs/:org_id/projects`

Create a project.

**Request**:
```json
{
  "name": "My Project",
  "description": "Optional description"
}
```

---

## Notes

### GET `/api/projects/:project_id/notes`

List notes in a project.

**Response** (200):
```json
[
  {
    "id": "uuid",
    "title": "Note Title",
    "kind": "markdown",
    "created_at": "2026-01-01T00:00:00Z",
    "updated_at": "2026-01-01T00:00:00Z"
  }
]
```

### GET `/api/projects/:project_id/notes/:note_id`

Get a specific note with content.

### POST `/api/projects/:project_id/notes`

Create a new note.

**Request**:
```json
{
  "title": "New Note",
  "kind": "markdown",
  "body": "# Hello\n\nContent here."
}
```

### PUT `/api/projects/:project_id/notes/:note_id`

Update note content.

### DELETE `/api/projects/:project_id/notes/:note_id`

Delete a note.

---

## Memberships

### GET `/api/orgs/:org_id/memberships`

List organization memberships.

### POST `/api/orgs/:org_id/memberships`

Add member to organization.

---

## Admin

### GET `/api/admin/users`

List all users (admin only).

### POST `/api/admin/invites`

Create an invite with temporary password.

**Request**:
```json
{
  "email": "newuser@example.com",
  "role": "member",
  "org_id": "uuid"
}
```

---

## Export/Import

### GET `/api/projects/:project_id/export`

Export project as ZIP archive.

**Response**: Binary ZIP file download.

### POST `/api/projects/:project_id/import`

Import notes from ZIP archive.

**Request**: Multipart form upload with ZIP file.

---

## Middleware & Security

| Middleware | Description |
|---|---|
| **Auth Extractor** | Validates Bearer token from `Authorization` header |
| **Permission Check** | RBAC enforcement per endpoint |
| **Audit Logging** | All write operations logged to `audit_log` table |
| **CORS** | Configured via `tower-http` |
| **Tracing** | Request/response logging via `tower-http` tracing |

---

## Error Response Format

All errors follow a consistent format:

```json
{
  "error": "Error description",
  "code": "ERROR_CODE"
}
```

| HTTP Status | Meaning |
|---|---|
| `400` | Bad request / validation error |
| `401` | Unauthorized (missing/invalid token) |
| `403` | Forbidden (insufficient permissions) |
| `404` | Resource not found |
| `409` | Conflict (duplicate resource) |
| `422` | Unprocessable entity |
| `500` | Internal server error |

---

## Configuration

| Variable | Default | Description |
|---|---|---|
| `OPN_BIND_ADDR` | `127.0.0.1:7878` | Listen address |
| `OPN_DB_PATH` | `./operon.db` | SQLite database path |
| `OPN_HOSTNAME` | `localhost` | Server hostname |
