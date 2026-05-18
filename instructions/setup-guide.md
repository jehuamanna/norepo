# Setup Guide

## Overview

Operon is a Rust/Dioxus application that targets three platforms:
- **Desktop** (default): Native app via Wry webview
- **Web**: WASM binary served in browser
- **Mobile**: Feature-gated (experimental)

This guide covers general setup. See platform-specific guides for OS details:
- [Windows](setup-windows.md)
- [Linux](setup-linux.md)
- [macOS](setup-macos.md)

---

## Prerequisites

| Requirement | Version | Purpose |
|---|---|---|
| Rust (stable) | Latest stable | Primary language |
| wasm32-unknown-unknown target | — | Web/WASM builds |
| Node.js | ≥20 | Playwright E2E + editor bridge |
| npm | ≥10 | Package management |
| dioxus-cli (`dx`) | Latest | Dev server, builds |
| just | Latest | Task runner |
| Chromium/Chrome | Latest | WASM tests + Playwright |
| clang | Latest | Only if using `wasm-sqlite` feature |

---

## Installation Steps

### 1. Clone the Repository

```bash
git clone <repo-url>
cd operon
```

### 2. Install Rust Toolchain

The project includes `rust-toolchain.toml` which automatically selects:
- Channel: `stable`
- Components: `clippy`, `rustfmt`
- Targets: `wasm32-unknown-unknown`

```bash
# Verify toolchain
rustup show
```

### 3. Install dioxus-cli

```bash
curl -sSL http://dioxus.dev/install.sh | sh
# OR
cargo install dioxus-cli
```

### 4. Install just

```bash
cargo install just
```

### 5. Bootstrap Everything

```bash
just bootstrap
```

This runs:
1. `cargo fetch` — download all Rust dependencies
2. `npm ci` — install Node.js dependencies (Playwright)
3. `npx playwright install --with-deps chromium` — install Playwright browser
4. Editor bridge build (bun/npm in `assets/editor-bridge/`)

### 6. Build Editor Bridge

```bash
just build-bridge
```

This compiles the TypeScript editor bridge (Monaco, CodeMirror 6, Tiptap) into ESM modules at `assets/editor-bridge/dist/`.

---

## Running the Application

### Desktop Mode (Default)

```bash
cargo run
```

Or with dioxus hot-reload:

```bash
dx serve
```

### Web Mode

```bash
dx serve --platform web --port 8123
```

Then open `http://localhost:8123` in a browser.

### API Server (Cloud Mode)

```bash
cargo run -p operon-api-server
```

Default: `http://127.0.0.1:7878`

Configure via environment variables:
- `OPN_BIND_ADDR` — Listen address (default `127.0.0.1:7878`)
- `OPN_DB_PATH` — SQLite path (default `./operon.db`)
- `OPN_HOSTNAME` — Server hostname (default `localhost`)

### CLI Agent

```bash
cargo run -p operon-agent-cli -- --provider anthropic --model claude-sonnet-4-6 --cwd . "describe this project"
```

---

## Environment Setup

### API Keys (Optional — for AI features)

Set environment variables for LLM providers:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GOOGLE_API_KEY="AIza..."
```

Or configure in the app's secret store (OS keyring on desktop).

### Configuration File (Optional)

Create `operon.toml` in the project root:

```toml
[runtime]
default_model = "claude-sonnet-4-6"

[providers.anthropic]
api_url = "https://api.anthropic.com"
model = "claude-sonnet-4-6"
max_tokens = 4096

[memory]
kind = "in_memory"
```

---

## Verification

### Run Unit Tests

```bash
just test-unit
```

### Run All Tests

```bash
just test-all
```

### Verify Desktop Build

```bash
cargo build --features desktop
```

### Verify Web Build

```bash
dx build --platform web
```

---

## Database

Operon uses **SQLite** with automatic migrations. No manual database setup required.

- **Desktop**: Database created automatically in the vault directory
- **API Server**: Database at `OPN_DB_PATH` (default `./operon.db`)
- **Web (wasm-sqlite)**: IndexedDB-backed OPFS virtual filesystem

### Migrations

16 migrations run automatically on first launch:
1. Core schema (users, orgs, projects, notes)
2. Password management fields
3. Audit logging
4. Note versioning
5. Local mode tables
6. Local projects/notes
7. Note kind fields
8. Image blob paths
9. Note links (wikilinks)
10. Project repo paths
11. Chat sessions/messages
12. Skill/Workflow/Artifact kinds

No manual migration commands needed.

---

## Next Steps

- [Development Guide](development-guide.md) — Daily workflow
- [Testing Guide](testing-guide.md) — 4-tier test strategy
- [Architecture](architecture.md) — System design
