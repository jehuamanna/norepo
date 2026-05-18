# Setup — macOS

## Prerequisites

### Install Homebrew

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

### Install Dependencies

```bash
brew install node       # Node.js ≥20
brew install just       # Task runner
brew install git        # Version control

# For wasm-sqlite feature (optional)
brew install llvm
```

---

## Step-by-Step Setup

### 1. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Verify
rustup show
# Should show: stable, clippy, rustfmt, wasm32-unknown-unknown
```

### 2. Install dioxus-cli

```bash
curl -sSL http://dioxus.dev/install.sh | sh
# Or:
cargo install dioxus-cli
```

### 3. Clone and Bootstrap

```bash
git clone <repo-url>
cd operon
just bootstrap
```

### 4. Build Editor Bridge

```bash
just build-bridge
```

---

## Apple Silicon Notes (M1/M2/M3/M4)

### Rust

Rust natively supports `aarch64-apple-darwin`. No special configuration needed — `rustup` installs the correct toolchain automatically.

### Node.js

Homebrew on Apple Silicon installs native ARM64 Node.js. No issues expected.

### LLVM/Clang (for wasm-sqlite)

If using `wasm-sqlite`, ensure LLVM is on PATH:

```bash
# Add to ~/.zshrc
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"
export LDFLAGS="-L/opt/homebrew/opt/llvm/lib"
export CPPFLAGS="-I/opt/homebrew/opt/llvm/include"
```

### Cross-Compilation

To build for Intel Macs from Apple Silicon:

```bash
rustup target add x86_64-apple-darwin
cargo build --target x86_64-apple-darwin
```

---

## Environment Variables

Add to `~/.zshrc` (default macOS shell):

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GOOGLE_API_KEY="AIza..."

# API Server (optional)
export OPN_BIND_ADDR="127.0.0.1:7878"
export OPN_DB_PATH="./operon.db"
export OPN_HOSTNAME="localhost"
```

Reload:

```bash
source ~/.zshrc
```

---

## Running

```bash
# Desktop app
cargo run

# Web dev server
dx serve --platform web --port 8123

# API server
cargo run -p operon-api-server

# CLI agent
cargo run -p operon-agent-cli -- --provider anthropic --model claude-sonnet-4-6 --cwd . "describe this project"
```

---

## Testing

```bash
just test-unit           # Tier 1
just test-integration    # Tier 2
just test-wasm           # Tier 3
just test-e2e            # Tier 4
just test-all            # All tiers
```

---

## macOS-Specific Notes

### Keychain for Secret Storage

The `keyring` crate uses **macOS Keychain** for secure API key storage. Secrets are stored in the login keychain — no additional setup required.

### Gatekeeper (First Run)

On first run, macOS may block the desktop app:
- Go to **System Settings → Privacy & Security**
- Click **Open Anyway** for the Operon binary

Or bypass from terminal:

```bash
xattr -cr target/debug/operon-dioxus
```

### File Watcher

The `notify` crate uses FSEvents on macOS. No limits to configure (unlike Linux inotify).

### Port Conflicts

macOS AirPlay Receiver uses port 5000. Operon uses port 8123 (web) and 7878 (API), so no conflict expected. If port 8123 is taken:

```bash
dx serve --platform web --port 9000
```

---

## Verification Checklist

- [ ] `rustup show` shows stable + wasm32-unknown-unknown
- [ ] `node --version` shows ≥20
- [ ] `just --version` works
- [ ] `dx --version` works
- [ ] `just bootstrap` completes
- [ ] `just build-bridge` completes
- [ ] `cargo build` succeeds
- [ ] `just test-unit` passes
- [ ] `dx serve --platform web --port 8123` starts
