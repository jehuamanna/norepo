# Coding Guidelines

## Naming Conventions

### Rust

| Item | Convention | Example |
|---|---|---|
| Crates | `operon-<name>` (kebab-case) | `operon-core`, `operon-store` |
| Modules | `snake_case` | `agent_runtime`, `file_ops` |
| Structs | `PascalCase` | `AgentRuntime`, `TabManager` |
| Traits | `PascalCase` | `ChatPlugin`, `Persistence` |
| Functions | `snake_case` | `load_note`, `save_bytes` |
| Constants | `SCREAMING_SNAKE_CASE` | `CRITICAL_HEAD`, `MAX_TOKENS` |
| Type aliases | `PascalCase` | `OperonResult<T>` |
| Enum variants | `PascalCase` | `Mode::Local`, `WatchEvent::Modified` |
| Feature flags | `kebab-case` | `wasm-sqlite`, `sqlite-memory` |

### TypeScript (Editor Bridge / E2E)

| Item | Convention | Example |
|---|---|---|
| Files | `kebab-case` | `codemirror.ts`, `smoke.spec.ts` |
| Classes | `PascalCase` | `AppShellPage` |
| Functions | `camelCase` | `createEditor`, `handleInput` |
| Constants | `SCREAMING_SNAKE_CASE` or `camelCase` | `DEFAULT_THEME` |
| Types/Interfaces | `PascalCase` | `EditorOptions` |

---

## Folder Conventions

### Crate Structure

```
crates/operon-<name>/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Public API exports
│   ├── error.rs        # Error types (if needed)
│   └── <modules>.rs    # Feature modules
```

### UI Component Structure (src/)

```
src/<feature>/
├── mod.rs              # Module root, re-exports
├── <component>.rs      # Individual Dioxus components
└── <subfeature>/       # Nested features
```

---

## Architecture Patterns

### Separation of Concerns

- **operon-core** and all `operon-plugins-*` crates must NOT depend on Dioxus (enforced by `deny.toml`)
- Persistence is bytes-only — format parsing belongs in format plugins
- State management uses Dioxus `Signal<T>` + `use_context_provider`/`use_context`

### Plugin Pattern

All extensibility points use Rust traits:
- `ChatPlugin` — LLM providers
- `ToolPlugin` — agent tools
- `FormatPlugin` — note format handlers
- `UIPlugin` — UI surface contributions
- `Persistence` — storage backends

Plugins are registered at compile time (no dynamic loading).

### Error Handling

- Each crate defines its own error type (e.g., `OperonError`, `StoreError`, `AuthError`, `ApiError`)
- Use `thiserror` for error derivation
- Return `Result<T, CrateError>` from public APIs
- Use `?` operator for propagation
- Map errors at crate boundaries

### Async Patterns

- Desktop: `tokio` multi-threaded runtime
- WASM: `wasm-bindgen-futures` for async
- Use `async-trait` for async trait methods
- Never hold `Signal` refs across `.await` (enforced by clippy.toml)

---

## Clean Code Rules

### Do

- Keep functions focused and small
- Use descriptive names — code should read like prose
- Prefer `Result<T>` over panicking
- Use `tracing` for logging (not `println!`)
- Use `cfg(test)` for inline unit tests
- Use `cfg(target_arch = "wasm32")` for platform-specific code

### Don't

- Don't use `unwrap()` in production code (use `?` or `expect()` with context)
- Don't hold signal refs across await points
- Don't depend on Dioxus from non-GUI crates
- Don't use `println!` for logging (use `tracing`)
- Don't use emoji/pictographs in source code (enforced by `no_pictographs` test)

---

## Linting Rules

### Clippy (clippy.toml)

```toml
await-holding-invalid-types = [
  "generational_box::GenerationalRef",
  "generational_box::GenerationalRefMut",
  "dioxus_signals::WriteLock",
]
```

**Why**: Holding Dioxus signal refs across `.await` points causes runtime panics. Clippy catches this at compile time.

### cargo-deny (deny.toml)

```toml
[[bans.deny]]
name = "dioxus"
wrappers = ["operon-dioxus"]
```

**Why**: Enforces architectural boundary — only the GUI crate touches Dioxus.

---

## Formatting Standards

- **Rust**: `cargo fmt` (default rustfmt config)
- **TypeScript**: Standard TypeScript formatting
- **Markdown**: Clean headings, tables, code blocks
- **TOML**: Consistent key ordering

Run before commit:

```bash
cargo fmt
cargo clippy --all-targets --all-features
```

---

## Commit Conventions

Recommended format:

```
<type>(<scope>): <description>

[optional body]
[optional footer]
```

### Types

| Type | Description |
|---|---|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation changes |
| `refactor` | Code refactoring |
| `test` | Test additions/changes |
| `chore` | Build, CI, dependencies |
| `perf` | Performance improvement |

### Scopes

| Scope | Description |
|---|---|
| `core` | operon-core changes |
| `store` | operon-store changes |
| `auth` | operon-auth changes |
| `api` | operon-api-server changes |
| `ui` | GUI component changes |
| `editor` | Editor bridge changes |
| `plugin` | Plugin system changes |
| `agent` | Agent runtime changes |
| `e2e` | E2E test changes |

### Examples

```
feat(editor): add CodeMirror 6 inline preview mode
fix(store): handle concurrent SQLite writes in WAL mode
refactor(core): extract budget tracking into separate module
test(e2e): add wikilink navigation spec
chore: update dioxus to 0.7.1
```

---

## Code Review Checklist

- [ ] No `unwrap()` in production paths
- [ ] No Dioxus dependency in core/plugin crates
- [ ] No signal refs held across `.await`
- [ ] `cargo fmt` applied
- [ ] `cargo clippy` clean
- [ ] Tests pass (`just test-unit` at minimum)
- [ ] Error handling uses `Result<T>` with proper types
- [ ] New public APIs are documented
