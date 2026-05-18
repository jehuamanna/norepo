# Documentation Coverage Report

> Run `node instructions/automation/coverage-checker.js` to regenerate.

*Last checked: 2026-05-14*

---

## Coverage by Category

### Crates Documentation

| Crate | Documented | Notes |
|---|---|---|
| `operon-core` | ✅ | architecture.md, how-it-works.md, folder-structure.md |
| `operon-store` | ✅ | database-schema.md, folder-structure.md |
| `operon-auth` | ✅ | security-guidelines.md, api-reference.md |
| `operon-api-server` | ✅ | api-reference.md, deployment-guide.md |
| `operon-notes` | ✅ | architecture.md, how-it-works.md |
| `operon-export` | ✅ | how-it-works.md, api-reference.md |
| `operon-plugins-anthropic` | ✅ | tech-stack.md, how-it-works.md |
| `operon-plugins-openai` | ✅ | tech-stack.md, how-it-works.md |
| `operon-plugins-google` | ✅ | tech-stack.md, how-it-works.md |
| `operon-plugins-claude-code` | ✅ | tech-stack.md, architecture.md |
| `operon-plugins-mcp` | ✅ | how-it-works.md, architecture.md |
| `operon-plugins-lsp` | ✅ | tech-stack.md, architecture.md |
| `operon-plugins-tools` | ✅ | architecture.md, how-it-works.md |
| `operon-agent-cli` | ✅ | setup-guide.md, build-guide.md |

**Coverage: 14/14 (100%)**

---

### API Routes Documentation

| Route Group | Documented in api-reference.md |
|---|---|
| Auth (login, signup, reset) | ✅ |
| Sessions | ✅ |
| User profile (me) | ✅ |
| Organizations | ✅ |
| Departments | ✅ |
| Teams | ✅ |
| Team members | ✅ |
| Team projects | ✅ |
| Projects | ✅ |
| Notes CRUD | ✅ |
| Memberships | ✅ |
| Admin users | ✅ |
| Admin invites | ✅ |
| Export/Import | ✅ |

**Coverage: 14/14 (100%)**

---

### Database Tables Documentation

| Table | Documented in database-schema.md |
|---|---|
| `users` | ✅ |
| `sessions` | ✅ |
| `orgs` | ✅ |
| `departments` | ✅ |
| `teams` | ✅ |
| `team_members` | ✅ |
| `projects` | ✅ |
| `team_projects` | ✅ |
| `notes` | ✅ |
| `note_updates` | ✅ |
| `attachments` | ✅ |
| `audit_log` | ✅ |
| `memberships` | ✅ |
| `chat_sessions` | ✅ |
| `chat_messages` | ✅ |
| `local_user` | ✅ |
| `local_app_settings` | ✅ |
| `local_project` | ✅ |
| `local_note` | ✅ |
| `local_note_link` | ✅ |
| `local_tree_state` | ✅ |
| `local_search` | ✅ |

**Coverage: 22/22 (100%)**

---

### Environment Variables Documentation

| Variable | Documented in environment-variables.md |
|---|---|
| `ANTHROPIC_API_KEY` | ✅ |
| `OPENAI_API_KEY` | ✅ |
| `GOOGLE_API_KEY` | ✅ |
| `OPN_BIND_ADDR` | ✅ |
| `OPN_DB_PATH` | ✅ |
| `OPN_HOSTNAME` | ✅ |
| `OPERON_RUNTIME__*` | ✅ |
| `OPERON_PROVIDERS__*` | ✅ |
| `OPERON_MEMORY__*` | ✅ |
| `OPERON_E2E_BASE_URL` | ✅ |
| `OPERON_E2E_HEADED` | ✅ |
| `CI` | ✅ |

**Coverage: 12/12 (100%)**

---

### Setup Guides

| Platform | Status |
|---|---|
| General | ✅ setup-guide.md |
| Windows | ✅ setup-windows.md |
| Linux | ✅ setup-linux.md |
| macOS | ✅ setup-macos.md |

**Coverage: 4/4 (100%)**

---

## Overall Coverage

| Category | Covered | Total | Percentage |
|---|---|---|---|
| Crates | 14 | 14 | 100% |
| API Routes | 14 | 14 | 100% |
| Database Tables | 22 | 22 | 100% |
| Environment Variables | 12 | 12 | 100% |
| Setup Guides | 4 | 4 | 100% |
| **Total** | **66** | **66** | **100%** |

---

## Potential Gaps

- [ ] Individual crate API documentation (rustdoc level) — not covered, use `cargo doc`
- [ ] MCP server configuration examples — partially covered in environment-variables.md
- [ ] Seed skills detailed documentation — referenced in how-it-works.md, each skill is self-documenting
- [ ] Editor bridge internal API — partially covered in folder-structure.md
