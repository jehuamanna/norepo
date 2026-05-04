#!/usr/bin/env bash
# Sync the wasm-pack cached chromedriver to match the local google-chrome
# major.minor version. Required because wasm-pack auto-downloads the latest
# ChromeDriver, which is incompatible with older system Chrome installs.
#
# Usage: ./scripts/sync-chromedriver.sh
# Exit codes: 0 on success or already-synced; non-zero on failure.
#
# Authored under the "Playwright for testing" Archon seed (84185cbf).
set -euo pipefail

if ! command -v google-chrome >/dev/null 2>&1; then
  echo "sync-chromedriver: google-chrome not on PATH; skipping" >&2
  exit 0
fi

CHROME_VERSION=$(google-chrome --version | awk '{print $3}')
CHROME_MAJOR_BUILD=$(echo "$CHROME_VERSION" | cut -d. -f1-3)  # e.g. 147.0.7727

WASM_PACK_CACHE_ROOT="${HOME}/.cache/.wasm-pack"
DRIVER_DIR=$(find "$WASM_PACK_CACHE_ROOT" -maxdepth 1 -type d -name 'chromedriver-*' 2>/dev/null | head -1)

if [[ -z "${DRIVER_DIR:-}" ]]; then
  echo "sync-chromedriver: wasm-pack chromedriver cache not found at $WASM_PACK_CACHE_ROOT" >&2
  echo "                   run wasm-pack test once to seed it, then re-run this script" >&2
  exit 0
fi

CACHED_DRIVER="$DRIVER_DIR/chromedriver"

if [[ -x "$CACHED_DRIVER" ]]; then
  CACHED_VERSION=$("$CACHED_DRIVER" --version | awk '{print $2}')
  CACHED_MAJOR_BUILD=$(echo "$CACHED_VERSION" | cut -d. -f1-3)
  if [[ "$CACHED_MAJOR_BUILD" == "$CHROME_MAJOR_BUILD" ]]; then
    echo "sync-chromedriver: already in sync (chromedriver $CACHED_VERSION matches chrome $CHROME_VERSION)"
    exit 0
  fi
  echo "sync-chromedriver: chromedriver $CACHED_VERSION does not match chrome $CHROME_VERSION; updating"
fi

# Pick the highest-patch chromedriver release matching the chrome major.minor.build
JSON_URL="https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json"
TMP=$(mktemp)
trap 'rm -f "$TMP" "$TMP.zip"' EXIT
curl -sSL "$JSON_URL" -o "$TMP"

DOWNLOAD_URL=$(python3 - "$TMP" "$CHROME_MAJOR_BUILD" <<'PY'
import json, sys
path, prefix = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
matches = [v for v in data["versions"] if v["version"].startswith(prefix + ".")]
if not matches:
    sys.exit(f"no chromedriver release for chrome version prefix {prefix}")
matches.sort(key=lambda v: list(map(int, v["version"].split("."))))
chosen = matches[-1]
for d in chosen["downloads"].get("chromedriver", []):
    if d["platform"] == "linux64":
        print(d["url"])
        sys.exit(0)
sys.exit(f"no linux64 chromedriver in release {chosen['version']}")
PY
)

echo "sync-chromedriver: downloading $DOWNLOAD_URL"
curl -sSL "$DOWNLOAD_URL" -o "$TMP.zip"
EXTRACT_DIR=$(mktemp -d)
trap 'rm -rf "$TMP" "$TMP.zip" "$EXTRACT_DIR"' EXIT
unzip -q -o "$TMP.zip" -d "$EXTRACT_DIR"
NEW_DRIVER=$(find "$EXTRACT_DIR" -name chromedriver -type f -executable | head -1)
[[ -n "$NEW_DRIVER" ]] || { echo "sync-chromedriver: no chromedriver in archive" >&2; exit 1; }
cp "$NEW_DRIVER" "$CACHED_DRIVER"
chmod +x "$CACHED_DRIVER"
echo "sync-chromedriver: $($CACHED_DRIVER --version)"
