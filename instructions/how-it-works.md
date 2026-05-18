# How It Works

## Application Startup

### Desktop Launch

1. **Tracing init**: `operon_dioxus::agent::tracing_init::init(None)` sets up structured logging
2. **Wry config**: Custom `bridge://` protocol handler registered for editor-bridge assets
3. **Critical CSS**: All 5 stylesheets (`main.css`, `tailwind.css`, `theme.css`, `shell.css`, `markdown.css`) are inlined via `include_str!` into the `<head>` — eliminates Flash of Unstyled Content (FOUC)
4. **Dioxus launch**: `LaunchBuilder::new().with_cfg(cfg).launch(App)` mounts the root component

### Web Launch

1. WASM binary loads in browser
2. `index.html` shows splash overlay (`#operon-splash`) while WASM initializes
3. Dark theme background prevents white flash
4. Dioxus hydrates into `#main` div
5. Splash overlay removed after first render

### App Component Initialization (`app.rs`)

The `App` component sets up all shared state via `use_context_provider`:

```
1. Theme → Signal<Theme> + ThemeRegistry
2. TabManager → Signal<TabManager>
3. EditorFocusRequest → Signal<Option<String>>
4. EditorRevealLine → Signal<Option<(String, u32)>>
5. Toast → Signal<Option<Toast>>
6. AppState → Signal<AppState> (mode: Local/NonLocal)
7. AboutDialog → Signal<bool>
8. RepoPermissions → Signal<bool> (desktop only)
9. LayoutState → Signal<LayoutState>
10. CommandRegistry → registered builtins
11. PluginRegistry → registered format + UI plugins
```

Then it determines the operating mode:
- **Desktop**: Reads vault root from `local_app_settings`; if none, shows `VaultDirPicker`
- **Web + wasm-sqlite**: Defaults to Local mode with OPFS persistence
- **Web without wasm-sqlite**: Defaults to NonLocal (cloud) mode

---

## Note Editing Workflow

### Creating a Note

1. User clicks "New Note" in explorer or uses command palette
2. A new `local_note` record is inserted into SQLite with a generated UUID
3. An empty file is created at `<vault>/notes/<id>.md`
4. A new tab opens in `TabManager`
5. The editor bridge loads the appropriate editor (Monaco/CM6/Tiptap based on note kind)

### Editing a Note

1. User types in the editor (JavaScript layer)
2. Editor bridge fires a custom event with the new content
3. Dioxus `oninput` handler receives the event
4. Content is buffered in `TabManager` (marks tab as dirty)
5. `SaveScheduler` debounces writes (prevents write storms)
6. On save trigger:
   - **Desktop**: `FilesystemPersistence::save()` writes to temp file, then atomic rename
   - **Web**: `OpfsPersistence::save()` writes to OPFS via wasm SQLite VFS
7. SQLite `local_note` metadata updated (modified timestamp, size)

### Auto-Save Debouncing

```
User types → 300ms idle → SaveScheduler triggers
User types again → timer resets
User types → 300ms idle → SaveScheduler triggers (batch save)
```

---

## Agent Runtime (ReAct Loop)

The agent runtime implements a **ReAct (Reason + Act) loop**:

```
1. User sends message
2. Runtime builds message history (system prompt + conversation)
3. ChatPlugin.chat(messages) → streaming LLM response
4. If LLM requests tool_use:
   a. PermissionGate checks if tool is allowed
   b. ToolPlugin.execute(tool_call) runs the tool
   c. Result appended to message history
   d. Loop back to step 3
5. If LLM returns text (no tool_use):
   a. Response streamed to UI
   b. Step marked as Done
6. Budget checked after each step:
   - Max tokens consumed?
   - Max steps reached?
   - Max tool calls reached?
   - Time limit exceeded?
7. If budget exhausted, loop terminates
```

### Streaming Events

The runtime emits a `Stream<Step>` with these event types:

| Event | Description |
|---|---|
| `Step::Started` | New agent turn begins |
| `Step::Thinking` | Extended thinking content (Anthropic) |
| `Step::Text(chunk)` | Streamed text token |
| `Step::ToolCall(call)` | Tool invocation request |
| `Step::ToolResult(result)` | Tool execution result |
| `Step::Done` | Turn complete |
| `Step::Error(err)` | Runtime error |

### Budget System

```rust
Budget {
    max_tokens: Option<u32>,      // Total tokens consumed
    max_seconds: Option<u64>,     // Wall-clock time
    max_tool_calls: Option<u32>,  // Total tool executions
    max_steps: Option<u32>,       // Total ReAct iterations
}
```

---

## Authentication Flow (Cloud Mode)

### Login

```
1. Client sends POST /api/auth/login { email, password }
2. Server looks up user in `users` table
3. Argon2 verifies password hash
4. Server generates session token (random bytes + SHA-256)
5. Token stored in `sessions` table
6. Token returned to client
7. Client stores token in AppState.session_token
8. Subsequent requests include token in Authorization header
```

### Session Validation

```
1. Request arrives with Authorization: Bearer <token>
2. Axum extractor looks up token in `sessions` table
3. If valid: extract user identity, attach to request
4. If expired/invalid: return 401 Unauthorized
```

### Password Reset

```
1. Admin creates invite with temporary password
2. User logs in with temp password
3. Server forces password change
4. New Argon2 hash stored
```

---

## Secret Management

### API Key Storage

Secrets (LLM API keys) are stored via the `SecretStore` trait:

| Backend | Platform | Storage |
|---|---|---|
| `KeyringSecretStore` | Desktop (macOS/Linux) | OS keyring (libsecret/Keychain) |
| `EnvSecretStore` | All | Environment variables |
| `JsonFileSecretStore` | Desktop (fallback) | Encrypted JSON file |

**Lookup order**: SecretStore → Environment variable → Error

### Supported Keys

- `ANTHROPIC_API_KEY` — Anthropic Claude API
- `OPENAI_API_KEY` — OpenAI / compatible endpoints
- `GOOGLE_API_KEY` — Google Gemini API

---

## Plugin System

### Format Plugins

Format plugins define how note content is parsed, rendered, and edited:

```
FormatPlugin trait:
  id()              → "markdown" | "code" | "richtext-tiptap"
  display_name()    → "Markdown" | "Code" | "Rich Text"
  detect(bytes)     → bool (content-type detection)
  capabilities()    → FormatCaps (what the plugin can do)
  language_descriptor() → LanguageDescriptor (editor config)
  can_import_from(other_id) → bool (conversion support)
```

Plugins are registered at compile time in `app.rs` — no dynamic loading.

### Agent Plugins

Agent plugins are categorized by trait:

- **ChatPlugin**: Sends messages to LLM, receives streaming response
- **ToolPlugin**: Executes tools (file ops, shell, git, web, LSP)
- **MemoryPlugin**: Persists conversation history

### MCP (Model Context Protocol)

MCP enables external tool integration:

```
1. StdioMcpClient spawns subprocess
2. JSON-RPC over stdin/stdout
3. McpToolProxy adapts MCP tools as ToolPlugin
4. Grant handlers control permissions:
   - AutoApproveGrantHandler (dev/testing)
   - DenyAllGrantHandler (restrictive)
   - SecretStoreGrantHandler (production)
```

---

## File Watcher (Desktop)

The `notify` crate watches the vault directory for external changes:

```
1. FilesystemPersistence registers watcher on vault root
2. File change detected → WatchEvent emitted
3. WatchEvent variants:
   - Modified(note_id) → reload content
   - Created(note_id) → add to explorer
   - Removed(note_id) → remove from explorer
   - Renamed { from, to } → update references
4. UI reacts to events via NoteWatcher::subscribe()
```

---

## Theme System

### Persistence

- **Web**: `WebLocalStorage` (browser localStorage)
- **Desktop**: Config file in app data directory

### Application

```
1. ThemeRegistry loads all built-in themes
2. User selects theme via command palette or settings
3. Theme signal updated → all reading components re-render
4. CSS custom properties applied to root element
5. Theme ID persisted for next session
```

---

## Seed Skills Pipeline

The seed skills system implements a cascading SDLC workflow:

```
Requirements
    ↓ 01-ba-discover-epics
Epics
    ↓ 01b-pm-prioritize-epics
Prioritized Backlog
    ↓ 02-ba-decompose-features
Features
    ↓ 03-ba-decompose-stories
Stories
    ↓ 04-ba-decompose-tasks
Tasks
    ↓ 05-sa-design-feature-hld
High-Level Design
    ↓ 06-sa-design-story-lld
Low-Level Design
    ↓ 07-sde-implement-task
Implementation
    ↓ 08-tst-write-tests
Test Cases
    ↓ 09-tst-run-tests
Test Results
    ↓ 10-sum-summarize-task
Summary
```

Each skill has defined `input_kind` and `output_kind` artifact types, with persona roles (BA, PM, SA, SDE, TST).

---

## Image Handling

### Paste from Clipboard (Desktop)

```
1. User pastes image (Ctrl/Cmd-V)
2. arboard crate reads clipboard image data
3. Image encoded as PNG (png crate)
4. SHA-256 hash computed for content addressing
5. Saved to <vault>/.operon/images/<sha>.<ext>
6. Markdown reference inserted: ![image](sha.png)
7. Editor renders inline preview
```

---

## Export/Import

### Export Flow

```
1. User triggers export (menu or command)
2. operon-export collects all notes + metadata
3. ZIP archive created with deflate compression
4. Each note: metadata JSON + body bytes
5. Content addressed with SHA-256
6. ZIP downloaded or saved to filesystem
```

### Import Flow

```
1. User selects ZIP file
2. operon-export extracts and validates
3. Conflict detection (existing notes with same ID)
4. Notes imported with metadata preservation
5. SQLite records created/updated
6. File explorer refreshed
```
