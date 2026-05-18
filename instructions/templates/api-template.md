# API Endpoint Template

## {METHOD} `{/api/path}`

**Description**: Brief description of what this endpoint does.

**Authentication**: Required / Not required

**Permissions**: Role(s) required (admin, member, viewer)

---

### Request

**Headers**:

| Header | Value | Required |
|---|---|---|
| `Authorization` | `Bearer <token>` | Yes |
| `Content-Type` | `application/json` | Yes |

**URL Parameters**:

| Parameter | Type | Required | Description |
|---|---|---|---|
| `id` | `string (UUID)` | Yes | Resource identifier |

**Query Parameters**:

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `page` | `integer` | No | `1` | Page number |
| `limit` | `integer` | No | `20` | Items per page |

**Request Body**:

```json
{
  "field_name": "string",
  "optional_field": "string | null"
}
```

| Field | Type | Required | Validation | Description |
|---|---|---|---|---|
| `field_name` | `string` | Yes | Max 255 chars | Description |
| `optional_field` | `string` | No | — | Description |

---

### Response

**Success (200)**:

```json
{
  "id": "uuid",
  "field_name": "value",
  "created_at": "2026-01-01T00:00:00Z"
}
```

**Success (201)** — Created:

```json
{
  "id": "uuid",
  "field_name": "value"
}
```

---

### Errors

| Status | Code | Description |
|---|---|---|
| `400` | `VALIDATION_ERROR` | Invalid request body |
| `401` | `UNAUTHORIZED` | Missing or invalid auth token |
| `403` | `FORBIDDEN` | Insufficient permissions |
| `404` | `NOT_FOUND` | Resource does not exist |
| `409` | `CONFLICT` | Resource already exists |
| `500` | `INTERNAL_ERROR` | Server error |

**Error Response Format**:

```json
{
  "error": "Human-readable error message",
  "code": "ERROR_CODE"
}
```

---

### Example

**Request**:

```bash
curl -X {METHOD} http://localhost:7878/api/path \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"field_name": "value"}'
```

**Response**:

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "field_name": "value"
}
```

---

### Implementation Notes

**Source**: `crates/operon-api-server/src/routes/{file}.rs`

**Middleware**: List applicable middleware (auth, rate limiting, etc.)

**Related**:
- [API Reference](../api-reference.md) — full API documentation
- [Database Schema](../database-schema.md) — underlying data model
