# Architecture

## System Overview

Operon follows a **layered, plugin-driven architecture** with strict separation of concerns enforced at the dependency level via `cargo-deny`. The system is divided into UI, runtime, persistence, auth, and API layers.

```mermaid
graph TB
    subgraph "GUI Layer"
        DX[operon-dioxus<br/>Dioxus 0.7 + Router]
        EB[Editor Bridge<br/>TS: Monaco / CM6 / Tiptap]
    end

    subgraph "Agent Runtime"
        CORE[operon-core<br/>ReAct Loop + Plugin System]
    end

    subgraph "LLM Plugins"
        ANT[operon-plugins-anthropic]
        OAI[operon-plugins-openai]
        GOO[operon-plugins-google]
        CC[operon-plugins-claude-code]
        MCP[operon-plugins-mcp]
        LSP[operon-plugins-lsp]
        TOOLS[operon-plugins-tools]
    end

    subgraph "Data Layer"
        STORE[operon-store<br/>SQLite + Migrations]
        NOTES[operon-notes<br/>Loro CRDT Versioning]
        EXPORT[operon-export<br/>ZIP Archives]
    end

    subgraph "Auth Layer"
        AUTH[operon-auth<br/>Argon2 + RBAC + Sessions]
    end

    subgraph "API Layer"
        API[operon-api-server<br/>Axum REST]
    end

    subgraph "CLI"
        CLI[operon-agent-cli]
    end

    DX --> CORE
    DX --> STORE
    DX --> EB
    CORE --> ANT & OAI & GOO & CC & MCP & LSP & TOOLS
    API --> AUTH & STORE & NOTES & EXPORT
    CLI --> CORE & ANT & OAI & GOO & TOOLS
```

---

## Module Boundaries

### Dependency Constraint (cargo-deny)

```toml
# deny.toml
[[bans.deny]]
name = "dioxus"
wrappers = ["operon-dioxus"]
reason = "operon-core and operon-plugins-* must remain UI-agnostic"
```

Only the root `operon-dioxus` crate may depend on Dioxus. All other crates (core, plugins, store, auth, api-server) are UI-agnostic and can be used independently.

### Crate Dependency Graph

```mermaid
graph LR
    DX[operon-dioxus] --> CORE[operon-core]
    DX --> STORE[operon-store]
    DX --> ANT[plugins-anthropic]
    DX --> MCP[plugins-mcp]
    DX --> CC[plugins-claude-code]

    API[operon-api-server] --> AUTH[operon-auth]
    API --> STORE
    API --> NOTES[operon-notes]
    API --> EXPORT[operon-export]

    CLI[operon-agent-cli] --> CORE
    CLI --> ANT
    CLI --> OAI[plugins-openai]
    CLI --> GOO[plugins-google]
    CLI --> TOOLS[plugins-tools]

    ANT --> CORE
    OAI --> CORE
    GOO --> CORE
    CC --> CORE
    MCP --> CORE
    LSP --> CORE
    TOOLS --> CORE

    NOTES --> STORE
    EXPORT --> STORE
    EXPORT --> NOTES
```

---

## Two-Mode Architecture

Operon operates in two mutually exclusive modes:

### Local Mode

```mermaid
graph LR
    UI[Dioxus UI] --> FS[Filesystem Persistence]
    UI --> SQLite[Local SQLite DB]
    UI --> Agent[Agent Runtime]
    Agent --> LLM[LLM Provider APIs]
    FS --> Vault[Vault Directory<br/>~/.operon or user-picked]
```

- **Desktop**: Full filesystem access, native directory picker (`rfd`), file watcher (`notify`), OS keyring for secrets
- **Web (wasm-sqlite)**: OPFS-backed persistence, IndexedDB for handles, wasm-compiled SQLite
- **No server required** — everything runs locally

### Cloud Mode (RBAG)

```mermaid
graph LR
    UI[Dioxus UI] --> API[Axum API Server]
    API --> SQLite[(SQLite DB)]
    API --> AUTH[Auth: Argon2 + Sessions]
    API --> RBAC[RBAC Engine]
    UI --> Agent[Agent Runtime]
    Agent --> LLM[LLM Provider APIs]
```

- **Server-backed**: Axum REST API handles data operations
- **Multi-user**: Organization → Department → Team → User hierarchy
- **Audit trail**: All operations logged

---

## Request Lifecycle

### Desktop Note Save Flow

```mermaid
sequenceDiagram
    participant U as User (Editor)
    participant T as TabManager
    participant S as SaveScheduler
    participant P as FilesystemPersistence
    participant DB as SQLite

    U->>T: Edit note content
    T->>S: Schedule save (debounced)
    S->>P: save(note_id, bytes)
    P->>P: Write temp file
    P->>P: Atomic rename
    P->>DB: Update local_note metadata
    DB-->>P: OK
    P-->>S: OK
    S-->>T: Mark clean
```

### Agent Chat Flow

```mermaid
sequenceDiagram
    participant U as User
    participant CP as Companion Panel
    participant RT as AgentRuntime
    participant CP2 as ChatPlugin (LLM)
    participant TP as ToolPlugin

    U->>CP: Send message
    CP->>RT: run(messages, budget)
    loop ReAct Loop
        RT->>CP2: chat(messages)
        CP2-->>RT: Stream tokens + tool_calls
        RT->>TP: execute(tool_call)
        TP-->>RT: ToolResult
        RT->>RT: Append to history
        RT->>RT: Check budget
    end
    RT-->>CP: Stream<Step> events
    CP-->>U: Render response
```

---

## Service Interactions

### Plugin System

```mermaid
graph TB
    subgraph "Format Plugins"
        MD[MarkdownPlugin]
        CODE[CodePlugin]
        RT[RichtextTiptapPlugin]
    end

    subgraph "UI Plugins"
        UIP[UIPlugin contributions]
    end

    subgraph "Agent Plugins"
        CP[ChatPlugin trait]
        TP[ToolPlugin trait]
        MP[MemoryPlugin trait]
    end

    REG[PluginRegistry] --> MD & CODE & RT & UIP
    AREG[AgentRegistry] --> CP & TP & MP
```

**Format Plugin trait**:
- `id()` — unique identifier (e.g., `"markdown"`)
- `detect(bytes)` — content-type detection
- `capabilities()` — supported operations
- `language_descriptor()` — editor language config

**Agent Plugin traits**:
- `ChatPlugin` — LLM conversation (streaming)
- `ToolPlugin` — tool execution (file, shell, git, web, LSP)
- `MemoryPlugin` — conversation history persistence

---

## Data Flow

### State Management (Dioxus)

```mermaid
graph TB
    APP[App Component] -->|use_context_provider| THEME[Signal&lt;Theme&gt;]
    APP -->|use_context_provider| TABS[Signal&lt;TabManager&gt;]
    APP -->|use_context_provider| STATE[Signal&lt;AppState&gt;]
    APP -->|use_context_provider| LAYOUT[Signal&lt;LayoutState&gt;]
    APP -->|use_context_provider| CMD[CommandRegistry]
    APP -->|use_context_provider| PLUG[PluginRegistry]

    SHELL[Shell] -->|use_context| THEME & TABS & STATE & LAYOUT
    EDITOR[EditorHost] -->|use_context| TABS
    EXPLORER[FileExplorer] -->|use_context| TABS & STATE
    COMPANION[CompanionPanel] -->|use_context| STATE
    PALETTE[CommandPalette] -->|use_context| CMD & TABS & THEME
```

All shared state is provided at the `App` root via `use_context_provider` and consumed by child components via `use_context`. Signals (`Signal<T>`) ensure reactive updates — writing to a signal automatically re-renders all components that read it.

---

## Frontend Architecture

### Editor Bridge

The editor bridge is a **TypeScript layer** that wraps three editor libraries and exposes a unified API to Dioxus via JavaScript interop:

```
Dioxus Component (Rust)
    ↕ JS eval / custom events
TypeScript Bridge (index.ts)
    ↕
Monaco | CodeMirror 6 | Tiptap
```

- **Desktop**: Loaded via custom `bridge://` Wry protocol
- **Web**: Bundled as ESM modules

### Layout System

```
┌─────────┬─────────────────────────┬──────────────┐
│Activity │                         │  Companion   │
│  Bar    │      Editor Area        │   Panel      │
│         │                         │  (Chat/AI)   │
│ [icons] │  ┌─────────────────┐    │              │
│         │  │  Tab Bar        │    │  ┌────────┐  │
│         │  ├─────────────────┤    │  │Sessions│  │
│         │  │                 │    │  │  Rail   │  │
│         │  │  Editor Content │    │  ├────────┤  │
│         │  │                 │    │  │  Chat   │  │
│         │  └─────────────────┘    │  │ Content │  │
│         │                         │  └────────┘  │
├─────────┴─────────────────────────┴──────────────┤
│                  Bottom Panel                     │
│           Logs | Problems | Terminal              │
└──────────────────────────────────────────────────┘
```

- **Sidebar**: 160–600px, collapsible
- **Companion**: 160–∞px, collapsible
- **Panel**: 96–600px, collapsible (default collapsed)
- **Session Rail**: 120–480px (inside companion)
- All regions drag-to-resize via `Splitter` component

---

## Persistence Architecture

```mermaid
graph TB
    subgraph "Persistence Trait"
        P[Persistence: load/save/list/delete/rename]
        W[NoteWatcher: subscribe to changes]
    end

    subgraph "Implementations"
        FS[FilesystemPersistence<br/>Desktop: atomic write-temp-rename]
        OPFS[OpfsPersistence<br/>Web: OPFS + wasm-sqlite]
        MEM[MemoryPersistence<br/>Tests: in-memory]
        WEB[WebPersistence<br/>Web: stub]
    end

    P --> FS & OPFS & MEM & WEB
    W --> FS
```

**Design principle**: Bytes-only storage. Format parsing (frontmatter, links, JSON) is the responsibility of format plugins, not the persistence layer.
