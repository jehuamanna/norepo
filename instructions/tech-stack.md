# Tech Stack

## Core Technologies

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Rust** | stable (via rust-toolchain.toml) | Primary language | Memory safety, performance, cross-compilation to WASM |
| **Dioxus** | 0.7.1 | UI framework | Rust-native, React-like, multi-platform (desktop/web/mobile) |
| **TypeScript** | 5.4 | Editor bridge | Required by editor libraries (Monaco, CodeMirror, Tiptap) |

## Frontend

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Dioxus Router** | 0.7.1 | Client-side routing | Type-safe enum-based routing, integrated with Dioxus |
| **Tailwind CSS** | (auto-compiled) | Utility-first CSS | Rapid UI development, auto-compiled by dioxus-cli |
| **Monaco Editor** | 0.50.0 | Source-text editing | VS Code's editor — full-featured, syntax highlighting |
| **CodeMirror 6** | v6 | Live-preview editing | Lightweight, extensible, inline Markdown rendering |
| **Tiptap** | 3.22.5 | Rich-text WYSIWYG | ProseMirror-based, clean API, extensible |
| **esbuild** | 0.21 | TypeScript bundling | Fast ESM bundling for editor bridge |

## Runtime & Async

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Tokio** | 1.x | Async runtime (desktop) | Industry standard, multi-threaded, full-featured |
| **wasm-bindgen** | — | WASM-JS interop | Required for browser APIs from Rust |
| **web-sys** | — | Browser API bindings | IndexedDB, OPFS, localStorage, MediaQuery access |
| **futures** | — | Async combinators | Stream processing for agent events |
| **async-trait** | — | Async trait support | Required for plugin trait definitions |
| **web-time** | 1.x | Browser-compatible time | `std::time` doesn't work in WASM |

## Desktop

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Wry** | (via Dioxus) | Webview runtime | Cross-platform webview (Chromium/WebKit), Tauri's renderer |
| **rfd** | 0.14 | Native file dialogs | Directory picker with xdg-portal support |
| **notify** | 6.x | Filesystem watcher | Detect external file changes in vault |
| **arboard** | 3.x | Clipboard access | Image paste support |
| **tempfile** | 3.x | Temp file creation | Atomic write-then-rename pattern |

## Persistence

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **rusqlite** | — | SQLite driver (desktop) | Zero-config embedded database, WAL mode |
| **r2d2** | — | Connection pool | Thread-safe connection reuse |
| **sqlite-wasm-rs** | — | SQLite in WASM | Browser-side SQLite (opt-in `wasm-sqlite` feature) |
| **Loro** | — | CRDT engine | Conflict-free note versioning, operational transform |
| **DashMap** | — | Concurrent HashMap | Lock-free concurrent data access |

## Authentication & Security

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Argon2** | — | Password hashing | Memory-hard, OWASP recommended |
| **SHA-256** | via `sha2` 0.10 | Content addressing | Image dedup, integrity verification |
| **keyring** | — | OS keyring (desktop) | Secure API key storage (macOS Keychain, Linux libsecret) |
| **rand** | — | Secure RNG | Session token generation |

## HTTP / API

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Axum** | — | HTTP server | Async, tower-based, Tokio-native |
| **Tower** | — | Service middleware | Rate limiting, CORS, tracing |
| **Reqwest** | 0.12 | HTTP client | LLM API calls, rustls-tls for security |
| **Lettre** | — | SMTP email | Invite flows, password reset |

## AI / Agent Runtime

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **Figment** | — | Configuration | TOML + env var layering (`operon.toml` + `OPERON_*`) |
| **MCP** (JSON-RPC) | — | Tool protocol | Extensible tool integration via subprocess |
| **lsp-types** | 0.95 | LSP protocol | Drive language servers (rust-analyzer, pyright, etc.) |
| **git2** | — | Git operations | Repository operations for agent tools |
| **globwalk/ignore** | — | File search | Glob patterns, respects .gitignore |

## Markdown

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **pulldown-cmark** | 0.10 | Markdown parsing | Fast, CommonMark-compliant, Rust-native |
| **@lezer/markdown** | 1.6.3 | CM6 Markdown parser | Required by CodeMirror live-preview mode |

## Export/Import

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **zip** | — | ZIP archives | Standard archive format for bulk export |

## Testing

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **cargo test** | — | Rust unit/integration tests | Built-in, fast |
| **wasm-pack** | — | WASM browser tests | Headless Chromium, wasm-bindgen-test |
| **Playwright** | 1.48+ | E2E browser tests | Cross-browser, reliable, auto-wait |
| **TypeScript** | 5.4 | E2E test language | Playwright's native language |

## Build Tools

| Technology | Version | Purpose | Why |
|---|---|---|---|
| **dioxus-cli** (`dx`) | — | Dev server & bundler | Hot reload, Tailwind compilation, WASM builds |
| **just** | — | Task runner | Cross-platform, Makefile alternative |
| **cargo-deny** | — | Dependency auditor | License, security, architectural enforcement |
| **Clippy** | — | Rust linter | Catch common mistakes, enforce patterns |
| **rustfmt** | — | Rust formatter | Consistent code style |

## Infrastructure

| Component | Details |
|---|---|
| **Database** | SQLite (embedded, WAL mode) |
| **Default Port (dev)** | 8123 (web), 7878 (API server) |
| **Targets** | `x86_64-*`, `aarch64-*`, `wasm32-unknown-unknown` |
| **Node.js** | ≥20 (for Playwright and editor-bridge build) |
