# Operon Developer Documentation

> **Operon** is a local-first, project-aware notes editor built with Rust, Dioxus 0.7, and a modular AI agent runtime. It features multi-mode editing (Monaco, CodeMirror 6, Tiptap), local SQLite persistence, and an integrated agent runtime supporting multiple LLM providers (Anthropic, OpenAI, Google Gemini).

---

## Quick Links

| Document | Description |
|---|---|
| [Architecture](architecture.md) | System design, module boundaries, data flow |
| [How It Works](how-it-works.md) | Application workflows and request handling |
| [Tech Stack](tech-stack.md) | Frameworks, libraries, and infrastructure |
| [Folder Structure](folder-structure.md) | Project layout and module responsibilities |
| [Setup Guide](setup-guide.md) | General setup overview |
| [Setup — Windows](setup-windows.md) | Windows-specific instructions |
| [Setup — Linux](setup-linux.md) | Linux-specific instructions |
| [Setup — macOS](setup-macos.md) | macOS-specific instructions |
| [Build Guide](build-guide.md) | Build pipeline and production builds |
| [Deployment Guide](deployment-guide.md) | Deployment architecture and strategies |
| [Development Guide](development-guide.md) | Local workflow, debugging, linting |
| [API Reference](api-reference.md) | REST endpoints, payloads, auth |
| [Database Schema](database-schema.md) | Tables, relationships, migrations |
| [Environment Variables](environment-variables.md) | All env vars with descriptions |
| [Coding Guidelines](coding-guidelines.md) | Naming, patterns, formatting standards |
| [Security Guidelines](security-guidelines.md) | Auth flow, secret management, OWASP |
| [Testing Guide](testing-guide.md) | 4-tier testing strategy |
| [Troubleshooting](troubleshooting.md) | Common issues and fixes |
| [Performance Optimization](performance-optimization.md) | Caching, optimization, scaling |
| [Future Improvements](future-improvements.md) | Technical debt and roadmap |
| [Requirements](requirements.md) | Business and functional requirements |
| [Changelog](changelog.md) | Version history and changes |
| [Coverage Report](coverage-report.md) | Documentation coverage tracking |
| [Docs Maintenance](docs-maintenance-guide.md) | How to maintain these docs |

---

## Onboarding Flow

1. **Read** [Architecture](architecture.md) to understand the system design
2. **Follow** the setup guide for your OS: [Windows](setup-windows.md) | [Linux](setup-linux.md) | [macOS](setup-macos.md)
3. **Run** `just bootstrap` to install all dependencies
4. **Launch** with `dx serve` (web) or `cargo run` (desktop)
5. **Read** [Development Guide](development-guide.md) for daily workflow
6. **Read** [Testing Guide](testing-guide.md) before writing tests

---

## Important Commands

```bash
# One-time setup
just bootstrap

# Development
dx serve                          # Web dev server (port 8123)
cargo run                         # Desktop app
just build-bridge                 # Rebuild TypeScript editor bridge

# Testing (4 tiers)
just test-unit                    # Tier 1: cargo test --lib
just test-integration             # Tier 2: cargo test --tests
just test-wasm                    # Tier 3: wasm-pack headless Chrome
just test-e2e                     # Tier 4: Playwright E2E
just test-all                     # All tiers, fail-fast

# Build
cargo build --release --features desktop   # Desktop binary
dx build --release --platform web          # Web WASM bundle
cargo build --release -p operon-api-server # API server
cargo build --release -p operon-agent-cli  # CLI agent
```

---

## Architecture Summary

```
┌─────────────────────────────────────────────────┐
│              operon-dioxus (GUI)                 │
│  Dioxus 0.7 + Router + Editor Bridge (TS)       │
│  Modes: Desktop (Wry) | Web (WASM) | Mobile     │
├─────────────────────────────────────────────────┤
│              operon-core (Runtime)               │
│  Agent Runtime (ReAct) | Plugin System           │
│  Budget | Sessions | Config | Secrets            │
├──────────┬──────────┬──────────┬────────────────┤
│ Plugins  │  Store   │  Auth    │  API Server    │
│ anthropic│  SQLite  │  Argon2  │  Axum REST     │
│ openai   │  CRDT    │  RBAC    │  RBAG/ODU/TPN  │
│ google   │  (Loro)  │  Sessions│  Export/Import │
│ mcp/lsp  │          │  Email   │                │
│ tools    │          │          │                │
└──────────┴──────────┴──────────┴────────────────┘
```

---

## Diagrams

- [Architecture Flow](diagrams/architecture-flow.md)
- [Request Lifecycle](diagrams/request-lifecycle.md)
- [Database Relations](diagrams/database-relations.md)
- [Deployment Flow](diagrams/deployment-flow.md)

---

## Automation

Documentation auto-sync tools are in [`automation/`](automation/):

- `doc-sync.js` — Scan repo and update docs incrementally
- `commit-parser.js` — Parse git commits and categorize changes
- `changelog-generator.js` — Generate structured changelog
- `coverage-checker.js` — Detect undocumented modules
- `structure-scanner.js` — Scan project structure and update map

Run with: `node instructions/automation/doc-sync.js`

---

*Last updated: 2026-05-14*
