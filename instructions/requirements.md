# Requirements

## Product Overview

Operon is a **local-first, project-aware notes editor** with an integrated AI agent runtime. It operates in two modes:

- **Local Mode**: Desktop-only with local SQLite + file system persistence (no server required)
- **Cloud Mode (RBAG)**: Role-based access with organization/department/user hierarchy, backed by an Axum HTTP API

---

## Business Requirements

### BR-1: Local-First Note Editing
Users must be able to create, edit, organize, and search notes without any network connection. All data stays on the user's machine in Local Mode.

### BR-2: Multi-Format Editing
Support three editing paradigms:
- **Source text** (Monaco) — for power users and code-heavy notes
- **Live preview** (CodeMirror 6) — Markdown with inline rendering
- **Rich text** (Tiptap) — WYSIWYG editing for non-technical users

### BR-3: AI Agent Integration
Provide an integrated AI assistant (companion panel) that can:
- Chat with context from the current project/notes
- Execute tools (file ops, shell, git, web search)
- Support multiple LLM providers (Anthropic, OpenAI, Google Gemini)
- Run via MCP (Model Context Protocol) for extensibility

### BR-4: Project-Aware Organization
Notes are organized within **projects** inside a **vault** (directory). The file explorer shows a hierarchical tree with drag-and-drop, context menus, and multi-select.

### BR-5: Artifact Pipeline (Seed Skills)
Provide a structured SDLC workflow: Requirements → Epics → Features → Stories → Tasks → HLD/LLD → Implementation → Tests → Summary. Each step is driven by a "skill" document with defined input/output artifact kinds.

### BR-6: Multi-Platform Support
- **Desktop** (Windows, macOS, Linux) via Wry/Tauri webview
- **Web** via WASM compilation
- **Mobile** (future, feature-gated)

### BR-7: Cloud Collaboration (RBAG Mode)
Organizations can deploy the Axum API server for multi-user collaboration with:
- Role-based access control (RBAC)
- Organization → Department → Team hierarchy
- Invite-based onboarding
- Audit logging

---

## Functional Requirements

### FR-1: Note CRUD
- Create notes (Markdown, Code, Richtext, Skill, Workflow, Artifact)
- Edit with auto-save (debounced `SaveScheduler`)
- Delete with undo support
- Rename notes
- Wikilink support (`[[note-name]]`)

### FR-2: File Explorer
- Hierarchical tree view with projects and notes
- Drag-and-drop reordering
- Context menu (new note, rename, delete, move-to submenu)
- Multi-select with Shift/Ctrl click
- Collapse/expand state persistence

### FR-3: Tab Management
- Multiple open notes in tabs
- Active tab tracking
- Close/close-all behavior
- Dirty indicator for unsaved changes

### FR-4: Command Palette
- Fuzzy search across commands, notes, and themes
- Keyboard shortcut: `Cmd/Ctrl-K`
- Live theme preview during theme picker mode

### FR-5: Theme System
- Multiple built-in color themes
- Theme persistence (localStorage on web, config file on desktop)
- Real-time theme switching

### FR-6: Agent Runtime
- ReAct loop with streaming events
- Budget tracking (tokens, steps, tool calls, time)
- Session continuity
- Tool approval/permission system
- Configurable via `operon.toml` + environment variables

### FR-7: Image Notes
- Paste images from clipboard
- Content-addressed storage (SHA-256)
- Display in notes with proper rendering

### FR-8: Search
- Full-text search across notes (FTS index in SQLite)
- Search panel with line-click-to-reveal
- Fuzzy note search in command palette

### FR-9: Export/Import
- Bulk export as ZIP archive
- Content-addressed with SHA-256
- Metadata + body separation
- Import with conflict resolution

### FR-10: Panel System
- Bottom panel with tabs: Logs, Problems, Terminal
- Collapsible with drag-to-resize
- Log buffer for in-app messages

---

## Non-Functional Requirements

### NFR-1: Performance
- Card flip animation < 200ms (Memory Game)
- WASM bundle optimized for fast load
- No white flash on desktop startup (critical CSS inlined)
- Debounced saves to prevent write storms

### NFR-2: Security
- Argon2 password hashing
- Session token-based authentication
- API key storage in OS keyring (desktop) or SecretStore
- Path traversal prevention in custom Wry protocol
- Cargo-deny enforces dependency constraints

### NFR-3: Reliability
- Atomic file writes (write-temp-rename on desktop)
- CRDT-based conflict resolution (Loro) for note versioning
- SQLite WAL mode for concurrent reads

### NFR-4: Accessibility
- WCAG AA compliance target
- Keyboard navigation support
- Color contrast requirements

### NFR-5: Extensibility
- Plugin system for format plugins and UI plugins
- MCP protocol support for external tool integration
- OpenAI-compatible API support (Ollama, vLLM, llama.cpp)

---

## User Roles

### Local Mode
- **Single User**: Full access to all features, no authentication required

### Cloud Mode (RBAG)
- **Master Admin**: Bootstrap admin, full system access
- **Org Admin**: Manage organization, departments, teams
- **Team Lead**: Manage team members and project access
- **Member**: Access assigned projects, create/edit notes
- **Viewer**: Read-only access to assigned projects

---

## Limitations

- Web mode without `wasm-sqlite` feature renders as "unavailable" for local mode
- Mobile support is feature-gated and not yet production-ready
- Cross-tab change notification not supported in web mode v1
- Editor bridge requires separate build step (`just build-bridge`)
- WASM SQLite requires `clang` on the host system
