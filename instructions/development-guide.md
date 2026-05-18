# Development Guide

## Daily Workflow

### Start Development

```bash
# Desktop with hot-reload
dx serve

# Web with hot-reload
dx serve --platform web --port 8123

# If editor bridge changed
just build-bridge
```

### Run Tests Before Committing

```bash
just test-unit           # Fast: ~5s
just test-integration    # Medium: ~15s
# Full suite if touching core logic:
just test-all
```

---

## Project Commands (Justfile)

| Command | Description |
|---|---|
| `just bootstrap` | One-time setup: fetch deps, npm ci, playwright install, build bridge |
| `just build-bridge` | Rebuild TypeScript editor bridge |
| `just test-unit` | `cargo test --lib` (inline unit tests) |
| `just test-integration` | `cargo test --tests` (tests/*.rs) |
| `just test-wasm` | `wasm-pack test --headless --chrome tests-wasm` |
| `just test-e2e` | `npx playwright test` |
| `just test-all` | All 4 tiers, fail-fast |
| `just test-e2e-ui` | Headed Playwright UI mode |
| `just e2e-report` | Open Playwright HTML report |
| `just sync-chromedriver` | Sync wasm-pack chromedriver with system Chrome |

---

## Hot Reload

Dioxus CLI provides hot-reload for RSX changes:

```bash
dx serve                          # Desktop
dx serve --platform web           # Web
```

**What hot-reloads**: RSX template changes (HTML structure, attributes, text)
**What requires rebuild**: Rust logic changes, new components, state management

### Editor Bridge Changes

The TypeScript editor bridge does NOT hot-reload. After changes to `assets/editor-bridge/`:

```bash
just build-bridge
# Then restart dx serve
```

---

## Debugging

### Rust Logging

The project uses `tracing` for structured logging:

```rust
tracing::debug!("loading note: {}", note_id);
tracing::info!("agent step completed");
tracing::warn!("budget nearly exhausted");
tracing::error!("persistence failed: {}", err);
```

Configure log level via `operon.toml`:

```toml
[runtime]
log_filter = "debug"
```

Or via environment variable:

```bash
OPERON_RUNTIME__LOG_FILTER=trace dx serve
```

### Browser DevTools (Web Mode)

1. Open `http://localhost:8123`
2. F12 → Console for WASM logs
3. Network tab for API calls
4. Application tab for IndexedDB/OPFS inspection

### Desktop DevTools

Wry includes Chromium DevTools:
- Right-click → Inspect (if enabled)
- Or set in Dioxus config

### Playwright Debug Mode

```bash
just test-e2e-ui          # Headed UI mode with time-travel
# Or:
OPERON_E2E_HEADED=1 npx playwright test --debug
```

---

## Code Generation

No code generation tools are used. All code is handwritten Rust/TypeScript.

---

## Linting

### Clippy

```bash
cargo clippy --all-targets --all-features
```

Custom rules in `clippy.toml`:

```toml
await-holding-invalid-types = [
  "generational_box::GenerationalRef",
  "generational_box::GenerationalRefMut",
  "dioxus_signals::WriteLock",
]
```

These prevent holding Dioxus signal refs across `.await` points (causes panics).

### cargo-deny

```bash
cargo deny check
```

Enforces:
- **Dioxus isolation**: Only `operon-dioxus` can depend on `dioxus`
- **License compliance**: Checks all dependency licenses
- **Security advisories**: Flags known vulnerabilities

### Rustfmt

```bash
cargo fmt --check       # Check formatting
cargo fmt               # Apply formatting
```

---

## Formatting

The project uses default `rustfmt` configuration. All Rust code must be formatted before commit:

```bash
cargo fmt
```

TypeScript (E2E tests, editor bridge) follows standard TypeScript formatting.

---

## Branch Strategy

Not enforced by tooling. Recommended:

```
main
├── feature/<name>     # New features
├── fix/<name>         # Bug fixes
├── refactor/<name>    # Refactoring
└── docs/<name>        # Documentation
```

---

## Adding a New Crate

1. Create directory under `crates/`:

```bash
cargo init crates/operon-new-crate --lib
```

2. Add to workspace in root `Cargo.toml`:

```toml
[workspace]
members = [
    # ...
    "crates/operon-new-crate",
]
```

3. If the crate must NOT depend on Dioxus, it's automatically enforced by `deny.toml`

---

## Adding a New Format Plugin

1. Implement `FormatPlugin` trait in `src/plugins/`
2. Register in `app.rs` plugin registry builder
3. Add note kind to `operon-store` migration (if new kind)

---

## Adding a New LLM Provider Plugin

1. Create crate `crates/operon-plugins-<name>/`
2. Implement `ChatPlugin` trait from `operon-core`
3. Add to `operon-agent-cli` as a `--provider` option
4. Add to GUI companion panel provider selector

---

## Adding a New Built-in Tool

1. Add to `crates/operon-plugins-tools/`
2. Implement `ToolPlugin` trait from `operon-core`
3. Register in `RuntimeAgentBackend` factory

---

## Working with SQLite Migrations

New migrations go in `crates/operon-store/src/migrations.rs`:

```rust
const MIGRATION_017_NEW_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS new_table (
        id TEXT PRIMARY KEY,
        -- ...
    );
"#;
```

Add to the migration runner array. Migrations run automatically on startup.

---

## Editor Bridge Development

The editor bridge lives in `assets/editor-bridge/`:

```
assets/editor-bridge/
├── package.json       # Dependencies (monaco, codemirror, tiptap)
├── tsconfig.json      # TypeScript config
├── index.ts           # Entry point + bridge protocol
├── monaco.ts          # Monaco backend
├── codemirror.ts      # CodeMirror 6 backend
├── tiptap.ts          # Tiptap backend
├── types.ts           # Shared types
└── dist/              # Built output (ESM)
```

After any change:

```bash
just build-bridge
```

The bridge communicates with Dioxus via:
- **Desktop**: `bridge://` custom Wry protocol → ESM imports
- **Web**: Direct ESM imports from bundled assets
- **IPC**: `window.postMessage` / custom events
