#!/usr/bin/env bash
# Build production artifacts.
#
# Usage:
#   ./scripts/build-release.sh --release <os> <formats…>
#
#   <os>       linux | macos | windows
#   <formats>  one or more (or `all`):
#                linux    →  deb appimage
#                macos    →  app dmg
#                windows  →  msi exe          (`exe` maps to NSIS)
#
# Examples:
#   ./scripts/build-release.sh --release linux all
#   ./scripts/build-release.sh --release linux deb
#   ./scripts/build-release.sh --release macos app dmg
#   ./scripts/build-release.sh --release windows msi exe
#
# Each OS must be its own build host. The script refuses to bundle for
# an OS that doesn't match the current `uname` — cross-bundling macOS
# from Linux is not viable (Apple SDK + codesigning). For a one-shot
# 3-OS pipeline, push and let `.github/workflows/release.yml` run the
# same script on three runners.
#
# Build-host prereqs:
#   Linux  (Debian/Ubuntu):
#     sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
#         libsoup-3.0-dev librsvg2-dev libssl-dev libsqlite3-dev \
#         pkg-config build-essential
#   macOS:                Xcode command-line tools (`xcode-select --install`)
#   Windows (Git Bash):   Visual Studio Build Tools + Rust MSVC toolchain

set -euo pipefail

# Resolve script path BEFORE `cd` — usage() reads the file later, by
# which point `$0` would be relative to the repo root rather than the
# scripts directory.
script_path="$(readlink -f "${BASH_SOURCE[0]}")"
cd "$(dirname "$script_path")/.."

usage() {
  sed -n '3,30p' "$script_path" >&2
  exit 2
}

# --- parse args ----------------------------------------------------------
[[ "${1:-}" == "--release" ]] || usage
shift
target_os="${1:-}"
shift || true
declare -a tokens=("$@")
(( ${#tokens[@]} )) || usage

case "$target_os" in
  linux|macos|windows) ;;
  *) echo "error: unknown OS '$target_os' (linux|macos|windows)" >&2; usage ;;
esac

# --- map (os, token) → dx --package-types value --------------------------
# `exe` is a Windows alias for the NSIS installer; `app` is the macOS
# bundle directory format. Everything else passes through verbatim.
declare -a allowed_for_linux=(deb appimage)
declare -a allowed_for_macos=(app dmg)
declare -a allowed_for_windows=(msi exe)

declare -a default_for_linux=(deb appimage)
declare -a default_for_macos=(macos dmg)
declare -a default_for_windows=(msi nsis)

declare -a wanted=()
declare -a passthrough=()
for tok in "${tokens[@]}"; do
  case "$tok" in
    --*)  passthrough+=("$tok") ;;
    all)
      case "$target_os" in
        linux)   wanted+=("${default_for_linux[@]}") ;;
        macos)   wanted+=("${default_for_macos[@]}") ;;
        windows) wanted+=("${default_for_windows[@]}") ;;
      esac ;;
    deb|appimage)
      [[ "$target_os" == linux ]] || { echo "error: '$tok' is a linux format" >&2; exit 2; }
      wanted+=("$tok") ;;
    app)
      [[ "$target_os" == macos ]] || { echo "error: 'app' is a macos format" >&2; exit 2; }
      wanted+=("macos") ;;
    dmg)
      [[ "$target_os" == macos ]] || { echo "error: 'dmg' is a macos format" >&2; exit 2; }
      wanted+=("dmg") ;;
    msi)
      [[ "$target_os" == windows ]] || { echo "error: 'msi' is a windows format" >&2; exit 2; }
      wanted+=("msi") ;;
    exe)
      [[ "$target_os" == windows ]] || { echo "error: 'exe' is a windows format" >&2; exit 2; }
      wanted+=("nsis") ;;
    *) echo "error: unknown format token '$tok'" >&2; usage ;;
  esac
done
(( ${#wanted[@]} )) || usage

# --- verify host matches target ------------------------------------------
host_os=""
case "$(uname -s)" in
  Linux*)               host_os=linux ;;
  Darwin*)              host_os=macos ;;
  MINGW*|MSYS*|CYGWIN*) host_os=windows ;;
  *) echo "error: unsupported host OS: $(uname -s)" >&2; exit 2 ;;
esac
if [[ "$host_os" != "$target_os" ]]; then
  cat >&2 <<EOF
error: cannot build for $target_os from a $host_os host.
       Each OS bundle must be produced on its own host (macOS needs the
       Apple SDK + codesigning; Windows needs Visual Studio Build Tools;
       Linux needs webkit2gtk-dev). Run this script on a $target_os
       machine, or push and let .github/workflows/release.yml fan out
       across all three CI runners.
EOF
  exit 2
fi

# --- pre-flight: pkg-config probe (Linux only) ---------------------------
if [[ "$target_os" == linux ]]; then
  echo "==> Pre-flight: pkg-config probe..."
  missing=()
  for pkg in webkit2gtk-4.1 gtk+-3.0 libsoup-3.0; do
    pkg-config --exists "$pkg" 2>/dev/null || missing+=("$pkg")
  done
  if (( ${#missing[@]} )); then
    cat >&2 <<EOF
warning: missing pkg-config entries: ${missing[*]}
  sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \\
                      librsvg2-dev libssl-dev libsqlite3-dev pkg-config
Continuing — some distros provide this metadata under different names.
EOF
  fi
fi

# --- rebuild the editor-bridge dist before cargo embeds it ---------------
# `src/main.rs` embeds `assets/editor-bridge/dist/` via `include_dir!`. Running
# `dx bundle` against a stale dist (or an empty one on a fresh checkout) ships
# a binary whose `bridge://` handler can't resolve `index.js` / `monaco-*.js`,
# so the Monaco host stays empty. `build.rs` re-emits `cargo:rerun-if-changed`
# for every file under dist, so refreshing it here forces cargo to rebuild
# `main.o` whenever the JS changed.
echo "==> Rebuilding editor-bridge dist (Monaco / CodeMirror / Tiptap shim)..."
( cd assets/editor-bridge && \
    if command -v bun >/dev/null 2>&1; then bun run build; \
    else npm run build; fi )

# --- build the sidecar binaries ------------------------------------------
echo "==> Building operon-mcp-permission + operon-posttool-hook (sidecars, release)..."
cargo build --release --bin operon-mcp-permission --bin operon-posttool-hook
# operon-mcp lives in a separate package (operon-bridge), so it needs its
# own -p invocation. It's the stdio MCP stub that lets chat-mode claude
# reach the in-tree operon-bridge socket — without it shipped, every
# mcp__operon_notes__* tool fails at startup with "operon-mcp binary not
# found" (see resolve_operon_mcp_bin in src/local_mode/bridge_runtime.rs).
echo "==> Building operon-mcp (operon-bridge stdio stub, release)..."
cargo build --release -p operon-bridge --bin operon-mcp

# Discover host triple so the dx `external_bin` lookup matches.
triple=$(rustc -vV | awk -F': ' '/^host:/ {print $2}')
[[ -n "$triple" ]] || { echo "error: could not detect rustc host triple" >&2; exit 2; }

shim_ext=""
if [[ "$target_os" == windows ]]; then
  shim_ext=".exe"
fi

declare -a staged_sidecars=()
for bin in operon-mcp-permission operon-posttool-hook operon-mcp; do
  src="target/release/${bin}${shim_ext}"
  dest="${bin}-${triple}${shim_ext}"
  echo "==> Staging ${bin} as ./${dest} for dx external_bin lookup..."
  cp -f "$src" "$dest"
  chmod +x "$dest" 2>/dev/null || true
  staged_sidecars+=("$dest")
done
trap 'rm -f "${staged_sidecars[@]}"' EXIT

# --- bundle --------------------------------------------------------------
pt_args=()
for t in "${wanted[@]}"; do
  pt_args+=(--package-types "$t")
done
echo "==> Bundling on ${host_os}: ${wanted[*]}"
dx bundle --release --platform desktop "${pt_args[@]}" "${passthrough[@]}"

echo
echo "==> Done. Artifacts:"
find target/dx -type f \
  \( -name '*.deb' -o -name '*.AppImage' -o -name '*.dmg' \
     -o -name '*.app' -o -name '*.msi' -o -name '*.exe' \) \
  -printf '  %p  (%s bytes)\n' 2>/dev/null || true

cat <<'EOF'

The target machine still needs the `claude` CLI:
  npm i -g @anthropic-ai/claude-code
EOF
