# Future Improvements

## Technical Debt

### TD-1: Web Persistence Stub
**Location**: `src/persistence/web.rs`
**Issue**: Without `wasm-sqlite` feature, web persistence is a stub that does nothing. Cross-tab change notification is also not supported.
**Impact**: Web mode without `wasm-sqlite` is effectively non-functional for local mode.
**Recommendation**: Either make `wasm-sqlite` the default for web builds or implement a proper IndexedDB-based persistence layer.

### TD-2: Page Object Model (E2E)
**Location**: `e2e/pages/AppShellPage.ts`
**Issue**: Page object model is a stub — most E2E tests interact directly with selectors.
**Impact**: Test maintenance burden increases as UI evolves.
**Recommendation**: Build out full page objects with reusable actions.

### TD-3: Plugin Dynamic Loading
**Issue**: All plugins are registered at compile time in `app.rs`. No runtime plugin loading.
**Impact**: Adding new format plugins or UI plugins requires recompilation.
**Recommendation**: Consider WASM-based dynamic plugin loading for extensibility.

### TD-4: Error Recovery in Agent Runtime
**Issue**: Agent runtime stops on first error. No retry or fallback logic.
**Impact**: Transient API failures terminate the entire agent session.
**Recommendation**: Add configurable retry with exponential backoff for transient errors.

---

## Scalability Improvements

### S-1: PostgreSQL Backend
**Current**: SQLite (single-file, single-writer)
**Goal**: Support 1000+ concurrent users
**Plan**: Implement PostgreSQL backend for `operon-store`, keeping SQLite for local/desktop mode.

### S-2: Real-Time Collaboration
**Current**: CRDT versioning (Loro) exists but is used for offline conflict resolution only.
**Goal**: Real-time multi-user editing with live cursors.
**Plan**: WebSocket server for CRDT delta broadcasting.

### S-3: Horizontal API Scaling
**Current**: Single API server instance.
**Goal**: Multiple API server instances behind load balancer.
**Dependency**: S-1 (PostgreSQL backend required first).

---

## Architecture Upgrades

### A-1: Mobile Support
**Current**: Feature-gated, not production-ready.
**Goal**: iOS and Android apps.
**Plan**: Dioxus mobile targets + platform-specific plugins.

### A-2: Offline-First Cloud Mode
**Current**: Cloud mode requires continuous server connectivity.
**Goal**: Work offline, sync when connected.
**Plan**: Local CRDT store + sync engine.

### A-3: Plugin Marketplace
**Current**: Built-in plugins only.
**Goal**: User-installable plugins (format plugins, UI plugins, tool plugins).
**Plan**: WASM plugin interface + distribution mechanism.

### A-4: Multi-Language LSP Integration
**Current**: LSP plugin supports one language server at a time.
**Goal**: Concurrent language servers for multi-language projects.
**Plan**: LSP multiplexer in `operon-plugins-lsp`.

---

## Pending Refactors

### R-1: Shell State Consolidation
**Issue**: Shell state is spread across multiple signals (`LayoutState`, `ShellState`, `CompanionState`).
**Goal**: Unified shell state management.

### R-2: Editor Bridge Protocol
**Issue**: Communication between Dioxus and editor bridge uses raw JS eval and custom events.
**Goal**: Typed message protocol with schema validation.

### R-3: Test Mock Consolidation
**Issue**: Mock plugins (echo, mock) are scattered across crates.
**Goal**: Centralized test utilities crate.

---

## Optimization Opportunities

### O-1: WASM Binary Size
**Current**: Full WASM binary includes all features.
**Goal**: < 2MB gzipped initial load.
**Plan**: Feature-flag optional components, aggressive tree shaking.

### O-2: Editor Bundle Splitting
**Current**: Editor bridge bundles all three editors.
**Goal**: Load only the active editor on demand.
**Plan**: Dynamic import for each editor backend.

### O-3: Incremental CRDT Sync
**Current**: Full document CRDT on each save.
**Goal**: Delta-only CRDT updates.
**Plan**: Loro already supports deltas — wire through persistence layer.

### O-4: Search Performance
**Current**: FTS5 search across all notes.
**Goal**: Sub-10ms search on 10,000+ notes.
**Plan**: Background indexing with incremental updates.

---

## Feature Wishlist

- [ ] Git integration in file explorer (status indicators, diff view)
- [ ] Markdown table editor
- [ ] Note templates (beyond skills)
- [ ] Custom keyboard shortcuts
- [ ] Split editor view
- [ ] Note versioning UI (diff viewer)
- [ ] Collaborative cursors
- [ ] Voice input for agent chat
- [ ] Image OCR (paste screenshot → extract text)
- [ ] Calendar/timeline view for notes
- [ ] Tag system with filtering
- [ ] Graph view (note connections)
