# Changelog

> Auto-generated changelog. Run `node instructions/automation/changelog-generator.js` to update.

---

## Format

Changes are grouped by type:
- **Features** — New functionality
- **Fixes** — Bug fixes
- **Refactors** — Code improvements without behavior changes
- **Tests** — Test additions or changes
- **Docs** — Documentation updates
- **Chores** — Build, CI, dependency updates

---

## Unreleased

*Run `node instructions/automation/changelog-generator.js` to populate from git log.*

---

## Initial Release (v0.1.0)

### Features

- Local-first note editing with three editor modes (Monaco, CodeMirror 6, Tiptap)
- AI agent runtime with ReAct loop and streaming
- Multi-provider LLM support (Anthropic, OpenAI, Google Gemini)
- Claude Code CLI integration via subprocess
- MCP (Model Context Protocol) support for external tools
- LSP integration for language intelligence
- Built-in tools: file ops, shell, git, web search/fetch, task management
- File explorer with drag-and-drop, multi-select, context menus
- Command palette with fuzzy search (commands, notes, themes)
- Tab management with auto-save
- Theme system with multiple built-in themes
- Wikilink support (`[[note-name]]`)
- Image notes with clipboard paste and SHA-256 content addressing
- Full-text search (FTS5)
- Export/import as ZIP archives
- CRDT-based note versioning (Loro)
- RBAG cloud mode with Axum API server
- RBAC with organization/department/team hierarchy
- Audit logging
- Seed skills pipeline (SDLC workflow automation)
- Desktop (Wry) and Web (WASM) targets
- 4-tier testing strategy (unit, integration, WASM, E2E)
