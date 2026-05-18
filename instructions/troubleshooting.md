# Troubleshooting

## Build Issues

### `error: linker 'cc' not found`

**Cause**: Missing C compiler.

**Fix**:
- **Linux**: `sudo apt install build-essential`
- **macOS**: `xcode-select --install`
- **Windows**: Install Visual Studio Build Tools with "Desktop development with C++"

---

### `error: failed to run custom build command for 'rusqlite'`

**Cause**: Missing SQLite development headers.

**Fix**:
- **Linux**: `sudo apt install libsqlite3-dev`
- **macOS**: Usually included with Xcode tools
- **Windows**: Bundled with rusqlite — should work automatically

---

### `error: wasm32-unknown-unknown target not found`

**Cause**: WASM target not installed.

**Fix**:
```bash
rustup target add wasm32-unknown-unknown
```

Note: `rust-toolchain.toml` should auto-install this. If not, the file may not be read (check working directory).

---

### `error: could not find 'clang'` (wasm-sqlite feature)

**Cause**: LLVM/Clang not installed (required for compiling SQLite to WASM).

**Fix**:
- **Linux**: `sudo apt install clang`
- **macOS**: `brew install llvm` + add to PATH
- **Windows**: `winget install LLVM.LLVM`

---

### Editor bridge build fails

**Cause**: Node.js dependencies not installed.

**Fix**:
```bash
cd assets/editor-bridge
npm install
cd ../..
just build-bridge
```

---

## Runtime Issues

### White flash on desktop startup

**Cause**: Critical CSS not loading.

**Check**: Ensure `assets/*.css` files exist and `CRITICAL_HEAD` in `main.rs` includes them via `include_str!`.

---

### `bridge://` protocol not loading editors

**Cause**: Editor bridge not built.

**Fix**:
```bash
just build-bridge
```

Verify `assets/editor-bridge/dist/index.js` exists.

---

### "Vault not found" on desktop

**Cause**: Previously selected vault directory was moved or deleted.

**Fix**: The app will show `VaultDirPicker` — select a new vault directory. Or clear the setting from SQLite:
```sql
DELETE FROM local_app_settings WHERE key = 'vault.root.path';
```

---

### Signal panic: "Cannot read signal while writing"

**Cause**: Holding a `Signal` ref across an `.await` point.

**Fix**: Clone the value before the `.await`:
```rust
// BAD
let val = signal.read();
some_async_fn().await;
use_val(*val);

// GOOD
let val = signal().clone();
some_async_fn().await;
use_val(val);
```

Clippy catches this — run `cargo clippy` to find violations.

---

### Agent not responding / "API key not found"

**Cause**: LLM API key not configured.

**Fix**: Set the environment variable:
```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

Or configure via the app's Settings → API Keys.

---

## Testing Issues

### WASM tests fail: "chromedriver version mismatch"

**Cause**: `wasm-pack` cached chromedriver doesn't match system Chrome version.

**Fix**:
```bash
just sync-chromedriver
# Or manually:
bash scripts/sync-chromedriver.sh
```

---

### Playwright tests timeout on first run

**Expected**: First run compiles WASM (~60s). Test timeout is 120s to accommodate this.

**If still timing out**: Check that `dx serve` starts successfully:
```bash
dx serve --platform web --port 8123
```

Wait for "Serving at http://localhost:8123" before running tests.

---

### Playwright `.only` blocked in CI

**Expected**: `CI=1` mode forbids `.only` to prevent accidental skipping.

**Fix**: Remove `.only` from test:
```typescript
// BAD
test.only('my test', async () => { ... });

// GOOD
test('my test', async () => { ... });
```

---

### `cargo deny check` fails

**Cause**: New dependency added that violates policy.

**Common violations**:
- **dioxus dependency in core/plugin crate**: Move the dependency to `operon-dioxus` only
- **License violation**: Check if the crate's license is compatible
- **Security advisory**: Update the affected dependency

---

## Database Issues

### "database is locked"

**Cause**: Multiple processes accessing same SQLite file without WAL mode.

**Fix**: SQLite should be in WAL mode by default. Check:
```bash
sqlite3 operon.db "PRAGMA journal_mode;"
# Should return: wal
```

---

### Migration failure

**Cause**: Schema change conflicts with existing data.

**Fix**: Backup and recreate:
```bash
cp operon.db operon.db.backup
rm operon.db
# Restart app — migrations will create fresh database
```

---

## Environment Issues

### Port 8123 already in use

**Fix**:
```bash
# Find the process
# Linux/macOS:
lsof -i :8123
# Windows:
netstat -ano | findstr 8123

# Use a different port
dx serve --platform web --port 9000
```

---

### WebView2 not found (Windows)

**Cause**: Missing Microsoft Edge WebView2 Runtime.

**Fix**:
```powershell
winget install Microsoft.EdgeWebView2Runtime
```

---

### File watcher exhausted (Linux)

**Cause**: inotify watch limit reached with large vaults.

**Fix**:
```bash
echo "fs.inotify.max_user_watches=524288" | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

---

## Debugging Tips

### Enable Verbose Logging

```bash
OPERON_RUNTIME__LOG_FILTER=trace cargo run
```

### Check Cargo Build Graph

```bash
cargo tree                        # Full dependency tree
cargo tree -p operon-core         # Single crate
cargo tree -i dioxus              # Inverse — who depends on dioxus?
```

### Inspect SQLite Database

```bash
sqlite3 operon.db
.tables                           # List tables
.schema local_note                # Show table schema
SELECT * FROM local_app_settings; # Query settings
```

### Profile Build Times

```bash
cargo build --timings
# Opens HTML report showing per-crate compile times
```
