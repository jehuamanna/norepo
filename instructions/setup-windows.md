# Setup — Windows

## Prerequisites

| Tool | Installation |
|---|---|
| **Rust** | [rustup-init.exe](https://rustup.rs/) or `winget install Rustlang.Rustup` |
| **Visual Studio Build Tools 2022** | `winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` |
| **Node.js ≥20** | [nodejs.org](https://nodejs.org/) or `winget install OpenJS.NodeJS.LTS` |
| **just** | `cargo install just` |
| **dioxus-cli** | `cargo install dioxus-cli --locked` |
| **Git** | [git-scm.com](https://git-scm.com/) or `winget install Git.Git` |
| **Chrome/Chromium** | Required for WASM tests and Playwright |

### Optional (for `wasm-sqlite` feature)
| Tool | Installation |
|---|---|
| **LLVM/Clang** | `winget install LLVM.LLVM` — required for compiling SQLite to WASM |

> **Important**: VS Build Tools with the C++ workload **must** be installed before
> `cargo install` or `cargo build` will work. Without it, Rust cannot find `link.exe`
> and all compilations fail with `error: linker 'link.exe' not found`.

---

## Step-by-Step Setup

### 1. Install Visual Studio Build Tools (do this FIRST)

The MSVC linker and C++ headers are required before any Rust compilation:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

> This downloads ~2–3 GB and installs silently. Wait for `Successfully installed`.

### 2. Install Rust

```powershell
winget install Rustlang.Rustup

# Reload PATH in current shell (or restart the terminal)
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

# Verify
rustup show
# Should show: stable-x86_64-pc-windows-msvc, clippy, rustfmt
rustc --version    # e.g. rustc 1.95.0
cargo --version    # e.g. cargo 1.95.0
```

The project's `rust-toolchain.toml` automatically provisions:
- `stable` channel
- `clippy`, `rustfmt` components
- `wasm32-unknown-unknown` target

### 3. Load the MSVC Developer Environment

After installing VS Build Tools, each new terminal session needs the MSVC
environment loaded. Run this **before** any `cargo` commands:

```powershell
# Load MSVC tools (cl.exe, link.exe) into the current shell
Import-Module "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" -SkipAutomaticLocation

# Also ensure cargo is on PATH
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

> **Tip**: Add these lines to your PowerShell profile (`$PROFILE`) so they
> run automatically in every terminal.

### 4. Install Node.js

```powershell
winget install OpenJS.NodeJS.LTS
# Verify
node --version   # ≥20
npm --version    # ≥10
```

### 5. Install Dev Tools

```powershell
cargo install just
cargo install dioxus-cli --locked
# Verify
just --version   # e.g. just 1.51.0
dx --version     # e.g. dioxus 0.7.9
```

### 6. Clone and Bootstrap

```powershell
git clone <repo-url>
cd operon

# Bootstrap all dependencies
just bootstrap
```

### 7. Build Editor Bridge

```powershell
just build-bridge
# Or manually:
cd assets\editor-bridge
npm install
npm run build
cd ..\..
```

---

## Environment Variables

Set via PowerShell (session):

```powershell
$env:ANTHROPIC_API_KEY = "sk-ant-..."
$env:OPENAI_API_KEY = "sk-..."
$env:GOOGLE_API_KEY = "AIza..."
```

Set permanently (User scope):

```powershell
[System.Environment]::SetEnvironmentVariable("ANTHROPIC_API_KEY", "sk-ant-...", "User")
[System.Environment]::SetEnvironmentVariable("OPENAI_API_KEY", "sk-...", "User")
[System.Environment]::SetEnvironmentVariable("GOOGLE_API_KEY", "AIza...", "User")
```

### API Server Variables

```powershell
$env:OPN_BIND_ADDR = "127.0.0.1:7878"
$env:OPN_DB_PATH = ".\operon.db"
$env:OPN_HOSTNAME = "localhost"
```

---

## Running

### Desktop App (Dev Mode)

```powershell
cargo run
# Or with hot-reload:
dx serve
```

### Desktop App (Release Build)

```powershell
dx build --release --platform desktop
```

Build output goes to:
```
target\dx\operon-dioxus\release\windows\app\
├── operon-dioxus.exe     (~35 MB)
└── assets\
    ├── favicon-*.ico
    ├── main-*.css
    ├── markdown-*.css
    ├── shell-*.css
    ├── tailwind-*.css
    └── theme-*.css
```

To copy the release build to a `build/` folder:

```powershell
$src = "target\dx\operon-dioxus\release\windows\app"
$dst = "build"
if (Test-Path $dst) { Remove-Item $dst -Recurse -Force }
Copy-Item $src -Destination $dst -Recurse
```

### Web Dev Server

```powershell
dx serve --platform web --port 8123
```

### API Server

```powershell
cargo run -p operon-api-server
```

### CLI Agent

```powershell
cargo run -p operon-agent-cli -- --provider anthropic --model claude-sonnet-4-6 --cwd . "describe this project"
```

---

## Testing

```powershell
just test-unit           # Tier 1: Unit tests
just test-integration    # Tier 2: Integration tests
just test-wasm           # Tier 3: WASM browser tests
just test-e2e            # Tier 4: Playwright E2E
just test-all            # All tiers
```

---

## Path Handling Notes

- Use forward slashes in `operon.toml` paths (Rust normalizes them)
- Vault paths support both `C:\Users\...` and `C:/Users/...`
- The `bridge://` custom protocol uses forward slashes internally

---

## Windows-Specific Issues

### `link.exe` Not Found / `linker not found`
This means the MSVC developer environment isn't loaded. Run:

```powershell
Import-Module "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" -SkipAutomaticLocation
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

If VS Build Tools isn't installed at all, install it first (see Step 1).

### `dx and dioxus versions are incompatible!`
This is a warning, not a fatal error. It occurs when `dioxus-cli` (dx) is a
slightly different version from the `dioxus` crate in `Cargo.toml`. The build
still succeeds. To silence it, match versions:

```powershell
cargo install dioxus-cli --version 0.7.1 --locked
```

### Claude Code Permission Bridge (Unix Sockets)
The `operon-plugins-claude-code` crate's permission bridge uses Unix domain
sockets, which are not available on Windows. The bridge compiles on Windows
as a stub — `PermissionBridge::bind()` returns an `Unsupported` error. This means
the inline permission-prompt MCP wiring is skipped — claude falls back to
its `--permission-mode` setting instead. All other claude-code functionality
works normally on Windows.

Similarly, the PostToolUse reload socket (`reload_socket`) is unavailable on
Windows; the inotify/filesystem watcher remains as the fallback for detecting
file changes made by Claude.

### WebView2 Runtime
Wry (desktop webview) requires **Microsoft Edge WebView2 Runtime**. It's pre-installed on Windows 10 (20H2+) and Windows 11. If missing:

```powershell
winget install Microsoft.EdgeWebView2Runtime
```

### Long Path Support
Enable long paths if your vault path is deeply nested:

```powershell
# Run as Administrator
New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" -Name "LongPathsEnabled" -Value 1 -PropertyType DWORD -Force
```

### WSL Notes
If using WSL for development:
- Desktop mode requires a Windows build (WSL can't access Wry)
- Web mode works in WSL — access via `http://localhost:8123` from Windows browser
- API server works in WSL

---

## Verification Checklist

- [ ] VS Build Tools installed: `& "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" -products *` returns a path
- [ ] MSVC environment loaded: `cl` prints compiler version
- [ ] `rustup show` shows stable-x86_64-pc-windows-msvc + wasm32-unknown-unknown
- [ ] `cargo --version` works (e.g. cargo 1.95.0)
- [ ] `node --version` shows ≥20
- [ ] `just --version` works
- [ ] `dx --version` works
- [ ] `just bootstrap` completes without errors
- [ ] `just build-bridge` (or `cd assets\editor-bridge && npm install && npm run build`) completes
- [ ] `dx build --release --platform desktop` succeeds — output in `target\dx\operon-dioxus\release\windows\app\`
- [ ] `operon-dioxus.exe` runs and shows the app window
- [ ] `just test-unit` passes
- [ ] `dx serve --platform web --port 8123` starts and opens in browser
