# Folder Structure

## Root Layout

```
operon/
├── Cargo.toml              # Workspace root + operon-dioxus package
├── Dioxus.toml             # Dioxus app config (bundle, web settings)
├── Justfile                # Task runner recipes (bootstrap, test, build)
├── package.json            # Node.js deps (Playwright E2E)
├── playwright.config.ts    # E2E test configuration
├── rust-toolchain.toml     # Stable Rust + wasm32 target
├── clippy.toml             # Clippy await-holding rules
├── deny.toml               # cargo-deny: Dioxus isolation enforcement
├── tailwind.css            # Tailwind entry point
├── index.html              # HTML shell (splash screen, critical styles)
├── tsconfig.json           # TypeScript config for E2E tests
│
├── src/                    # Main Dioxus GUI application
├── crates/                 # Workspace crate library
├── assets/                 # CSS, editor bridge, static files
├── tests/                  # Rust integration tests (Tier 2)
├── tests-wasm/             # WASM browser tests (Tier 3)
├── e2e/                    # Playwright E2E tests (Tier 4)
├── scripts/                # Utility shell scripts
├── seed-skills/            # SDLC skill templates
├── seed-skills-employee/   # Employee-variant skill templates
├── seed-skills-sum/        # Summary-variant skill templates
├── seed-skills-updated/    # Updated skill templates
├── seed-skills-updated-memory/ # Memory-enhanced skill templates
├── memory-game/            # Memory game spec (demo/test project)
└── instructions/           # This documentation
```

---

## `src/` — Main Application

The root crate (`operon-dioxus`) contains the Dioxus GUI application.

| Path | Description |
|---|---|
| `main.rs` | Desktop entry point: Wry config, `bridge://` protocol, critical CSS |
| `lib.rs` | Library exports (16 modules) |
| `app.rs` | Root `App` component: context providers, mode detection, shell mount |

### `src/agent/`
Agent runtime integration for the GUI layer.

| File | Description |
|---|---|
| `tracing_init.rs` | Structured logging setup (desktop: tracing-subscriber, wasm: tracing-wasm) |

### `src/commands/`
Command registry and command palette implementation.

| File | Description |
|---|---|
| `mod.rs` | `CommandRegistry`, `Command`, `CommandContext` types |
| `builtins.rs` | Built-in command registration (save, delete, toggle sidebar, etc.) |
| `palette.rs` | Command palette UI: fuzzy search, three modes (commands/notes/themes) |
| `fuzzy.rs` | Fuzzy string matching algorithm |

### `src/editor/`
Editor backend trait and three implementations wrapping JavaScript editor libraries.

| File | Description |
|---|---|
| `mod.rs` | `EditorBackend` trait, `EditorCommand` enum |
| `monaco.rs` | Monaco editor integration (source-text mode) |
| `codemirror.rs` | CodeMirror 6 integration (live-preview mode) |
| `tiptap.rs` | Tiptap integration (rich-text WYSIWYG mode) |

### `src/local_mode/`
Local mode (desktop + wasm-sqlite) implementation.

| Path | Description |
|---|---|
| `mod.rs` | Module root, feature-gate routing |
| `vault.rs` | Vault directory abstraction |
| `vault_picker.rs` | Modal directory picker (rfd native dialog) |
| `wasm_init.rs` | WASM initialization (wasm-sqlite feature only) |
| `wasm_shell.rs` | WASM shell layout (wasm-sqlite feature only) |
| `wasm_stub.rs` | Stub when wasm-sqlite is off |
| `web_vault_handle.rs` | IndexedDB for OPFS handle persistence |
| `desktop/` | Desktop-specific: file operations, vault directory handling |
| `editor/` | Local mode editor components |
| `explorer/` | File explorer tree, selected project/note state |
| `images/` | Image note handling (clipboard paste, SHA-256 addressing) |
| `ui/` | Shared UI components (toast, fallback views) |

### `src/shell/`
Main application shell (layout, menubar, activity bar, companion panel).

| File | Description |
|---|---|
| `layout.rs` | `LayoutState`: sidebar/companion/panel widths + collapse flags |
| `menubar.rs` | Menu bar component |
| `state.rs` | Activity bar items, last-active tracking |
| `about.rs` | About dialog |
| `splitter.rs` | Drag-to-resize regions |
| `companion_state.rs` | Chat session management |
| `repo_permissions.rs` | Desktop-only: `.claude/settings.local.json` management |
| `mcp_settings/` | MCP server configuration UI |

### `src/persistence/`
Note storage abstraction (trait-based, bytes-only).

| File | Description |
|---|---|
| `mod.rs` | `Persistence` and `NoteWatcher` traits, `WatchEvent` enum |
| `fs.rs` | Desktop: atomic write-temp-rename, `notify` file watcher |
| `opfs.rs` | Web: OPFS-backed (wasm-sqlite feature) |
| `memory.rs` | In-memory (tests) |
| `web.rs` | Web: stub (no wasm-sqlite) |

### `src/plugin/`
Plugin system: registry, traits, manifest.

| File | Description |
|---|---|
| `mod.rs` | Module root |
| `traits.rs` | `FormatPlugin`, `UIPlugin` traits |
| `registry.rs` | `PluginRegistry` (compile-time registration) |
| `manifest.rs` | Plugin metadata definitions |
| `context.rs` | Plugin execution context |

### `src/plugins/`
Built-in format and UI plugin implementations (Markdown, Code, Richtext).

### `src/tabs/`
Tab manager and save scheduler.

### `src/theme/`
Theme registry, theme definitions, persistence (localStorage / config file).

### `src/panel/`
Bottom panel: Logs, Problems, Terminal tabs.

### `src/log/`
In-app log buffer for message display.

### `src/problems/`
Diagnostics and linter output display.

### `src/rbag/`
RBAG mode state (`AppState`, `Mode` enum: Local/NonLocal).

### `src/ui/`
Reusable Dioxus UI components.

### `src/util/`
Helpers (slug generation, markdown utilities).

---

## `crates/` — Workspace Crates

| Crate | Description |
|---|---|
| `operon-core` | UI-agnostic agent runtime: ReAct loop, plugin traits, budget, config, secrets |
| `operon-store` | SQLite persistence substrate: migrations, repositories, RBAC tables |
| `operon-auth` | Authentication: Argon2 passwords, sessions, invites, RBAC, email |
| `operon-api-server` | Axum HTTP REST API for cloud mode |
| `operon-notes` | Note versioning with Loro CRDT |
| `operon-export` | Export/import as ZIP archives with SHA-256 |
| `operon-plugins-anthropic` | Anthropic Claude Messages API (streaming SSE) |
| `operon-plugins-openai` | OpenAI Chat Completions + compatible endpoints |
| `operon-plugins-google` | Google Gemini API (streaming SSE) |
| `operon-plugins-claude-code` | Claude Code CLI subprocess driver |
| `operon-plugins-mcp` | Model Context Protocol client (JSON-RPC over stdio) |
| `operon-plugins-lsp` | LSP client (rust-analyzer, pyright, etc.) |
| `operon-plugins-tools` | Built-in tools: file, shell, git, web, task, patch |
| `operon-agent-cli` | Standalone CLI agent driver |

---

## `assets/` — Static Assets

| Path | Description |
|---|---|
| `main.css` | Core grid layout styles |
| `tailwind.css` | Tailwind CSS output (auto-compiled) |
| `theme.css` | Color palette CSS custom properties |
| `shell.css` | Shell chrome styles (menubar, splitters, panels) |
| `markdown.css` | Markdown rendering styles |
| `editor-bridge/` | TypeScript editor bridge project |
| `editor-bridge/index.ts` | Entry point + bridge protocol |
| `editor-bridge/monaco.ts` | Monaco editor backend |
| `editor-bridge/codemirror.ts` | CodeMirror 6 backend |
| `editor-bridge/tiptap.ts` | Tiptap editor backend |
| `editor-bridge/types.ts` | Shared TypeScript types |
| `editor-bridge/dist/` | Built output (ESM modules) |

---

## `tests/` — Integration Tests (Tier 2)

| File | What's Tested |
|---|---|
| `agent_runtime.rs` | ReAct loop, budget exhaustion, echo plugins |
| `agent_mcp.rs` | MCP tool proxy integration |
| `agent_runtime_permission.rs` | Permission gate, glob-based checks |
| `markdown.rs` | Markdown parsing and rendering |
| `theme_registry.rs` | Theme loading and switching |
| `vault.rs` | Vault directory operations |
| `plugin_registry.rs` | Plugin registration and lookup |
| `shell_state.rs` | Shell state management |
| `menubar.rs` | Menu bar construction |
| `theme_palettes.rs` | Color palette validation |
| `no_pictographs.rs` | Emoji/pictograph usage enforcement |

---

## `e2e/` — Playwright E2E Tests (Tier 4)

| Path | Description |
|---|---|
| `specs/` | Test specifications |
| `pages/` | Page Object Model (AppShellPage) |
| `fixtures/` | Test data and setup |
| `utils/` | Test utilities |

### E2E Test Specs

| Spec | What's Tested |
|---|---|
| `smoke.spec.ts` | Page loads, title check |
| `theme-picker.spec.ts` | Theme switching |
| `sidebar-collapse.spec.ts` | Sidebar toggle |
| `note-create.spec.ts` | Note creation flow |
| `multi-select.spec.ts` | Multi-selection in explorer |
| `image-notes.spec.ts` | Image note creation |
| `explorer-dnd.spec.ts` | Drag-and-drop in explorer |
| `explorer-undo.spec.ts` | Undo operations |
| `wikilinks.spec.ts` | Wikilink navigation |
| `editor-auto-focus.spec.ts` | Editor focus behavior |
| `explorer-context-menu-submenu.spec.ts` | Context menu submenus |
| `mode-toolbar.spec.ts` | Editor mode switching |

---

## `seed-skills/` — SDLC Skill Templates

10 numbered Markdown files defining the artifact pipeline:

```
01-ba-discover-epics.md          → Requirements → Epics (BA persona)
01b-pm-prioritize-epics.md       → Epics → Prioritized Backlog (PM)
02-ba-decompose-features.md      → Epic → Features (BA)
02b-pm-prioritize-features.md    → Features → Prioritized Backlog (PM)
03-ba-decompose-stories.md       → Feature → Stories (BA)
03b-pm-prioritize-stories.md     → Stories → Prioritized Backlog (PM)
04-ba-decompose-tasks.md         → Story → Tasks (BA)
04b-pm-prioritize-tasks-coarse.md → Tasks → Prioritized Backlog (PM)
05-sa-design-feature-hld.md      → Feature → HLD Plan (SA)
06-sa-design-story-lld.md        → Story → LLD Plan (SA)
06b-pm-prioritize-tasks-refined.md → Tasks → Refined Backlog (PM)
06c-sa-prioritize-plans.md       → Plans → Prioritized Plans (SA)
07-sde-implement-task.md         → Task → Implementation (SDE)
08-tst-write-tests.md            → Implementation → Test Cases (TST)
09-tst-run-tests.md              → Test Cases → Test Results (TST)
10-sum-summarize-task.md         → Test Results → Summary (Summary)
```

---

## `scripts/` — Utility Scripts

| Script | Purpose |
|---|---|
| `sync-chromedriver.sh` | Sync wasm-pack chromedriver cache with system Chrome version |
