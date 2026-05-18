# Performance Optimization

## Current Optimizations

### Zero FOUC (Flash of Unstyled Content)

**Desktop**: All 5 CSS files are embedded at compile-time via `include_str!` into `CRITICAL_HEAD` and injected synchronously into the `<head>` before first render.

**Web**: `index.html` includes a splash overlay (`#operon-splash`) with dark theme background, preventing white flash while WASM loads.

### Debounced Auto-Save

The `SaveScheduler` batches writes:
- User types → 300ms idle → single save operation
- Prevents write storms during active editing
- Desktop: atomic write-temp-rename (no partial writes)

### Atomic File Writes

Desktop persistence uses write-temp-rename:
1. Write to temporary file
2. Atomic rename to target path
3. No data corruption on crash/power loss

### SQLite WAL Mode

WAL (Write-Ahead Logging) enables:
- Concurrent reads during writes
- Better write performance
- Reduced lock contention

### Connection Pooling

`r2d2` connection pool for SQLite:
- Reuses connections across requests
- Thread-safe connection management
- Configurable pool size

### Content Addressing

Images are SHA-256 hashed for:
- Deduplication (same image → same hash → stored once)
- Integrity verification
- Cache-friendly paths

---

## Caching

### Theme Cache

- Themes loaded once into `ThemeRegistry`
- Persisted via `WebLocalStorage` (web) or config file (desktop)
- No recomputation on theme switch

### Editor Bridge Cache

- ESM modules cached by browser (web) or Wry protocol (desktop)
- Only rebuilt when TypeScript source changes

### WASM Binary Cache

- Browser caches WASM binary after first load
- Subsequent loads are near-instant

---

## Lazy Loading

### Format Plugins

Format plugins are registered at compile time but only initialized when a note of that format is opened.

### Editor Libraries

The editor bridge loads only the active editor backend:
- Monaco loaded only when source-text mode is selected
- CodeMirror loaded only when live-preview mode is selected
- Tiptap loaded only when rich-text mode is selected

---

## Database Optimization

### Indexes

SQLite migrations create indexes on:
- `notes.project_id` — fast lookup by project
- `local_note.project_id` — fast lookup by project
- `sessions.token` — fast session validation
- `audit_log.created_at` — fast audit queries
- `local_search` — FTS5 full-text search index

### Query Optimization

- Repository layer uses parameterized queries (no string interpolation)
- Batch operations where possible
- FTS5 for full-text search (not LIKE %query%)

---

## Bundle Optimization

### WASM Size

```toml
[profile.release]
opt-level = "z"    # Optimize for size
lto = true         # Link-time optimization
codegen-units = 1  # Single codegen unit
strip = true       # Strip debug info
```

Post-processing:
```bash
wasm-opt -Oz dist/*.wasm -o dist/optimized.wasm
```

### Editor Bridge Bundle

esbuild with:
- Tree shaking (dead code elimination)
- ESM format (enables browser-native module loading)
- ES2022 target (modern syntax, smaller output)
- Code splitting (separate chunks for each editor)

---

## Scaling Strategies

### Desktop

Desktop mode is single-user, single-machine. Scaling is limited by:
- SQLite: Handles thousands of notes without issues
- File system: Limited by OS file descriptor limits
- Memory: Dioxus virtual DOM is memory-efficient

### Web (WASM)

- WASM binary size is the primary constraint
- Use `wasm-sqlite` for local persistence (no server needed)
- Browser cache eliminates repeated downloads

### API Server

SQLite limits:
- **~100 concurrent users** comfortably
- **WAL mode** enables concurrent reads
- **Connection pool** manages thread safety

For higher scale:
- Replace `operon-store` backend with PostgreSQL
- Add read replicas
- Use reverse proxy for load balancing

---

## Performance Bottlenecks

### Known

| Bottleneck | Impact | Mitigation |
|---|---|---|
| Initial WASM compile | ~60s first load | Browser caching after first load |
| Editor bridge size | Large JS bundle | Code splitting per editor backend |
| SQLite single-writer | Limits concurrent writes | WAL mode, connection pool |
| CRDT merge on large notes | Slow for very large documents | Loro optimized for common cases |

### Monitoring

```bash
# Build time profiling
cargo build --timings

# Runtime tracing
OPERON_RUNTIME__LOG_FILTER=trace cargo run

# SQLite analysis
sqlite3 operon.db "EXPLAIN QUERY PLAN SELECT ..."
```

---

## Recommendations

### For Large Vaults (1000+ notes)

- Enable FTS5 search (default in local mode)
- Increase inotify limits on Linux
- Use SSD for vault directory

### For Production API Server

- Use reverse proxy with caching
- Enable SQLite WAL mode (default)
- Monitor connection pool utilization
- Set `OPERON_RUNTIME__LOG_FILTER=info` (reduce log volume)

### For CI/CD

- Cache `target/` directory between builds
- Cache `~/.cargo/registry/` and `~/.cargo/git/`
- Cache `node_modules/` for Playwright
- Use `cargo build --timings` to identify slow crates
