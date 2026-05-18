# Setup — Linux

## Prerequisites

### Debian/Ubuntu

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev libsqlite3-dev \
    libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
    librsvg2-dev libxdo-dev curl git

# Node.js ≥20
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt install -y nodejs

# Chrome/Chromium (for WASM tests + Playwright)
sudo apt install -y chromium-browser
# Or: sudo apt install -y google-chrome-stable
```

### Fedora/RHEL

```bash
sudo dnf install -y gcc gcc-c++ openssl-devel sqlite-devel \
    gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel \
    librsvg2-devel libxdo-devel curl git

# Node.js ≥20
sudo dnf install -y nodejs

# Chrome/Chromium
sudo dnf install -y chromium
```

### Arch Linux

```bash
sudo pacman -S base-devel openssl sqlite gtk3 webkit2gtk-4.1 \
    libappindicator-gtk3 librsvg libxdotool curl git nodejs npm chromium
```

### Optional (for `wasm-sqlite` feature)

```bash
# Debian/Ubuntu
sudo apt install -y clang

# Fedora
sudo dnf install -y clang

# Arch
sudo pacman -S clang
```

---

## Step-by-Step Setup

### 1. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Verify (rust-toolchain.toml auto-installs components)
rustup show
```

### 2. Install Dev Tools

```bash
cargo install just
cargo install dioxus-cli
# Or for dx:
curl -sSL http://dioxus.dev/install.sh | sh
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

## Environment Variables

### Shell Configuration (bash/zsh)

Add to `~/.bashrc` or `~/.zshrc`:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GOOGLE_API_KEY="AIza..."

# API Server (optional)
export OPN_BIND_ADDR="127.0.0.1:7878"
export OPN_DB_PATH="./operon.db"
export OPN_HOSTNAME="localhost"
```

Then reload:

```bash
source ~/.bashrc  # or source ~/.zshrc
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
just test-wasm           # Tier 3 (needs Chrome/Chromium)
just test-e2e            # Tier 4
just test-all            # All tiers
```

### ChromeDriver Sync

If WASM tests fail due to ChromeDriver version mismatch:

```bash
just sync-chromedriver
# Or manually:
bash scripts/sync-chromedriver.sh
```

---

## Linux-Specific Notes

### Keyring for Secret Storage

The `keyring` crate uses `libsecret` (GNOME Keyring) or `kwallet` (KDE):

```bash
# GNOME
sudo apt install -y libsecret-1-dev gnome-keyring

# KDE
sudo apt install -y kwallet
```

If no keyring daemon is running (headless/CI), secrets fall back to environment variables.

### File Watcher Limits

The `notify` crate uses inotify. For large vaults, increase the watcher limit:

```bash
echo "fs.inotify.max_user_watches=524288" | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

### XDG Portal (rfd)

The native directory picker uses xdg-desktop-portal:

```bash
# GNOME
sudo apt install -y xdg-desktop-portal-gnome

# KDE
sudo apt install -y xdg-desktop-portal-kde

# wlroots (Sway/Hyprland)
sudo apt install -y xdg-desktop-portal-wlr
```

### Headless/Server Environments

For CI or headless servers (no display):
- Desktop mode won't work (needs X11/Wayland)
- Web mode works — access remotely via SSH tunnel or network
- API server works fully headless
- CLI agent works fully headless

---

## Permissions

The vault directory needs read/write permissions:

```bash
# Default vault location
mkdir -p ~/.operon
chmod 700 ~/.operon
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
