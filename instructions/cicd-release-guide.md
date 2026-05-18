# CI/CD, Releases & Auto-Updates — GitHub Guide

> Comprehensive guide to setting up GitHub Actions CI/CD, managing versioned
> releases, and wiring desktop auto-updates for the Operon project.

---

## Table of Contents

1. [Current CI Landscape](#current-ci-landscape)
2. [Repository Secrets & Variables](#repository-secrets--variables)
3. [CI Workflow — Tests](#ci-workflow--tests)
4. [CD Workflow — Release Build & Publish](#cd-workflow--release-build--publish)
5. [Release Strategy](#release-strategy)
6. [Version Bumping](#version-bumping)
7. [Desktop Auto-Updates](#desktop-auto-updates)
8. [Web Deployment](#web-deployment)
9. [API Server Deployment](#api-server-deployment)
10. [Branch Protection & Merge Rules](#branch-protection--merge-rules)
11. [Maintenance & Troubleshooting](#maintenance--troubleshooting)

---

## Current CI Landscape

The project already has a test-only workflow at `.github/workflows/test.yml`:

| Job | Runner | What it does |
|-----|--------|-------------|
| `unit-and-integration` | `ubuntu-latest` | `just test-unit` + `just test-integration` |
| `wasm-tests` | `ubuntu-latest` | `just test-wasm` (wasm-pack + headless Chromium) |
| `e2e` | Playwright container | `just test-e2e` (Playwright against `dx serve --platform web`) |

**Missing**: release builds, cross-platform packaging, auto-updates, deploy pipelines.

---

## Repository Secrets & Variables

### Required Secrets

Configure these in **Settings → Secrets and variables → Actions**:

| Secret | Purpose | Example |
|--------|---------|---------|
| `SIGNING_CERTIFICATE_BASE64` | Windows code-signing cert (PFX, base64-encoded) | `base64 -w0 cert.pfx` |
| `SIGNING_CERTIFICATE_PASSWORD` | PFX passphrase | — |
| `APPLE_CERTIFICATE_BASE64` | macOS signing cert (p12, base64-encoded) | — |
| `APPLE_CERTIFICATE_PASSWORD` | p12 passphrase | — |
| `APPLE_ID` | Apple Developer email | `dev@example.com` |
| `APPLE_APP_PASSWORD` | App-specific password for notarization | — |
| `APPLE_TEAM_ID` | 10-char team identifier | `ABCDE12345` |
| `DEPLOY_SSH_KEY` | SSH key for web/API server deploy | — |
| `RELEASE_TOKEN` | GitHub PAT with `contents: write` (or use `GITHUB_TOKEN`) | — |

### Repository Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `APP_VERSION` | Current release version (optional — prefer Cargo.toml) | `0.1.0` |
| `UPDATE_SERVER_URL` | URL where update manifests are hosted | `https://releases.operon.dev` |

---

## CI Workflow — Tests

The existing `.github/workflows/test.yml` runs on every push to `main` and
every PR. **No changes needed** — it already covers all four test tiers.

Trigger summary:

```yaml
on:
  pull_request:
  push:
    branches: [main]
```

---

## CD Workflow — Release Build & Publish

Create `.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    tags:
      - "v*.*.*"       # e.g. v0.2.0, v1.0.0-beta.1

permissions:
  contents: write       # Create GitHub Releases + upload assets

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false   # Never cancel a release in progress

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  NODE_VERSION: "20"

jobs:
  # ──────────────────────────────────────────────────────────
  # 1. Extract version from the tag
  # ──────────────────────────────────────────────────────────
  meta:
    runs-on: ubuntu-latest
    outputs:
      version: ${{ steps.ver.outputs.version }}
      prerelease: ${{ steps.ver.outputs.prerelease }}
    steps:
      - id: ver
        run: |
          TAG="${GITHUB_REF#refs/tags/v}"
          echo "version=$TAG" >> "$GITHUB_OUTPUT"
          if [[ "$TAG" == *"-"* ]]; then
            echo "prerelease=true" >> "$GITHUB_OUTPUT"
          else
            echo "prerelease=false" >> "$GITHUB_OUTPUT"
          fi

  # ──────────────────────────────────────────────────────────
  # 2. Build desktop bundles (Windows, macOS, Linux)
  # ──────────────────────────────────────────────────────────
  build-desktop:
    needs: meta
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact: operon-windows-x64
            ext: .exe
          - os: macos-latest
            target: aarch64-apple-darwin
            artifact: operon-macos-arm64
            ext: ""
          - os: macos-13
            target: x86_64-apple-darwin
            artifact: operon-macos-x64
            ext: ""
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact: operon-linux-x64
            ext: ""

    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }},wasm32-unknown-unknown
          components: clippy, rustfmt

      - uses: Swatinem/rust-cache@v2
        with:
          key: release-${{ matrix.target }}

      - uses: actions/setup-node@v4
        with:
          node-version: ${{ env.NODE_VERSION }}

      # Linux: install system deps for Wry/WebKitGTK
      - name: Install Linux dependencies
        if: runner.os == 'Linux'
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            libwebkit2gtk-4.1-dev \
            libgtk-3-dev \
            libayatana-appindicator3-dev \
            librsvg2-dev \
            patchelf

      # Install dx (dioxus-cli)
      - name: Install dioxus-cli
        run: cargo install dioxus-cli --locked

      # Build editor bridge
      - name: Build editor bridge
        run: |
          cd assets/editor-bridge
          npm install
          npm run build

      # Release build
      - name: Build desktop release
        run: dx build --release --platform desktop

      # Windows: sign the executable
      - name: Sign Windows binary
        if: runner.os == 'Windows' && env.SIGNING_CERTIFICATE_BASE64 != ''
        env:
          SIGNING_CERTIFICATE_BASE64: ${{ secrets.SIGNING_CERTIFICATE_BASE64 }}
          SIGNING_CERTIFICATE_PASSWORD: ${{ secrets.SIGNING_CERTIFICATE_PASSWORD }}
        shell: pwsh
        run: |
          $cert = [System.Convert]::FromBase64String($env:SIGNING_CERTIFICATE_BASE64)
          $certPath = Join-Path $env:TEMP "cert.pfx"
          [System.IO.File]::WriteAllBytes($certPath, $cert)
          $exe = Get-ChildItem -Path "target/dx/operon-dioxus/release/windows/app" -Filter "*.exe" -Recurse | Select-Object -First 1
          & signtool sign /f $certPath /p $env:SIGNING_CERTIFICATE_PASSWORD /tr http://timestamp.digicert.com /td sha256 /fd sha256 $exe.FullName
          Remove-Item $certPath -Force

      # macOS: sign and notarize
      - name: Sign & notarize macOS binary
        if: runner.os == 'macOS' && env.APPLE_CERTIFICATE_BASE64 != ''
        env:
          APPLE_CERTIFICATE_BASE64: ${{ secrets.APPLE_CERTIFICATE_BASE64 }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_APP_PASSWORD: ${{ secrets.APPLE_APP_PASSWORD }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
        run: |
          # Import certificate
          echo "$APPLE_CERTIFICATE_BASE64" | base64 --decode > cert.p12
          security create-keychain -p "" build.keychain
          security import cert.p12 -k build.keychain -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
          security set-key-partition-list -S apple-tool:,apple: -s -k "" build.keychain
          security default-keychain -s build.keychain
          security unlock-keychain -p "" build.keychain

          # Sign
          APP_PATH="target/dx/operon-dioxus/release/macos/app"
          codesign --force --deep --sign "Developer ID Application: $APPLE_TEAM_ID" "$APP_PATH/operon-dioxus"

          # Notarize
          zip -r operon.zip "$APP_PATH"
          xcrun notarytool submit operon.zip \
            --apple-id "$APPLE_ID" \
            --password "$APPLE_APP_PASSWORD" \
            --team-id "$APPLE_TEAM_ID" \
            --wait
          rm cert.p12

      # Package into archive
      - name: Package release
        shell: bash
        run: |
          VERSION="${{ needs.meta.outputs.version }}"
          ARTIFACT="${{ matrix.artifact }}"

          if [ "$RUNNER_OS" = "Windows" ]; then
            SRC="target/dx/operon-dioxus/release/windows/app"
            7z a "${ARTIFACT}-${VERSION}.zip" "./${SRC}/*"
          elif [ "$RUNNER_OS" = "macOS" ]; then
            SRC="target/dx/operon-dioxus/release/macos/app"
            tar czf "${ARTIFACT}-${VERSION}.tar.gz" -C "$(dirname $SRC)" "$(basename $SRC)"
          else
            SRC="target/dx/operon-dioxus/release/linux/app"
            tar czf "${ARTIFACT}-${VERSION}.tar.gz" -C "$(dirname $SRC)" "$(basename $SRC)"
          fi

      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact }}
          path: |
            *.zip
            *.tar.gz
          retention-days: 7

  # ──────────────────────────────────────────────────────────
  # 3. Build web bundle (optional — for hosted deployment)
  # ──────────────────────────────────────────────────────────
  build-web:
    needs: meta
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown

      - uses: Swatinem/rust-cache@v2
        with:
          key: release-web

      - uses: actions/setup-node@v4
        with:
          node-version: ${{ env.NODE_VERSION }}

      - name: Install dioxus-cli
        run: cargo install dioxus-cli --locked

      - name: Build editor bridge
        run: cd assets/editor-bridge && npm install && npm run build

      - name: Build web release
        run: dx build --release --platform web

      - uses: actions/upload-artifact@v4
        with:
          name: operon-web
          path: target/dx/operon-dioxus/release/web/public/
          retention-days: 7

  # ──────────────────────────────────────────────────────────
  # 4. Build API server binary
  # ──────────────────────────────────────────────────────────
  build-api-server:
    needs: meta
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact: operon-api-server-linux-x64
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact: operon-api-server-windows-x64

    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: Swatinem/rust-cache@v2
        with:
          key: api-${{ matrix.target }}

      - name: Build API server
        run: cargo build --release -p operon-api-server

      - name: Package
        shell: bash
        run: |
          VERSION="${{ needs.meta.outputs.version }}"
          if [ "$RUNNER_OS" = "Windows" ]; then
            7z a "${{ matrix.artifact }}-${VERSION}.zip" "target/release/operon-api-server.exe"
          else
            tar czf "${{ matrix.artifact }}-${VERSION}.tar.gz" -C target/release operon-api-server
          fi

      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact }}
          path: |
            *.zip
            *.tar.gz

  # ──────────────────────────────────────────────────────────
  # 5. Create GitHub Release with all assets
  # ──────────────────────────────────────────────────────────
  publish-release:
    needs: [meta, build-desktop, build-web, build-api-server]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/download-artifact@v4
        with:
          path: artifacts/

      - name: Generate release notes
        id: notes
        run: |
          # Extract commits since last tag
          PREV_TAG=$(git describe --tags --abbrev=0 HEAD^ 2>/dev/null || echo "")
          if [ -n "$PREV_TAG" ]; then
            NOTES=$(git log "${PREV_TAG}..HEAD" --pretty=format:"- %s (%h)" --no-merges)
          else
            NOTES="Initial release"
          fi
          echo "notes<<EOF" >> "$GITHUB_OUTPUT"
          echo "$NOTES" >> "$GITHUB_OUTPUT"
          echo "EOF" >> "$GITHUB_OUTPUT"

      - name: Generate update manifest
        run: |
          VERSION="${{ needs.meta.outputs.version }}"
          cat > artifacts/update-manifest.json << 'MANIFEST'
          {
            "version": "${{ needs.meta.outputs.version }}",
            "notes": "See https://github.com/${{ github.repository }}/releases/tag/v${{ needs.meta.outputs.version }}",
            "platforms": {
              "windows-x64": {
                "url": "https://github.com/${{ github.repository }}/releases/download/v${{ needs.meta.outputs.version }}/operon-windows-x64-${{ needs.meta.outputs.version }}.zip",
                "signature": ""
              },
              "macos-arm64": {
                "url": "https://github.com/${{ github.repository }}/releases/download/v${{ needs.meta.outputs.version }}/operon-macos-arm64-${{ needs.meta.outputs.version }}.tar.gz",
                "signature": ""
              },
              "macos-x64": {
                "url": "https://github.com/${{ github.repository }}/releases/download/v${{ needs.meta.outputs.version }}/operon-macos-x64-${{ needs.meta.outputs.version }}.tar.gz",
                "signature": ""
              },
              "linux-x64": {
                "url": "https://github.com/${{ github.repository }}/releases/download/v${{ needs.meta.outputs.version }}/operon-linux-x64-${{ needs.meta.outputs.version }}.tar.gz",
                "signature": ""
              }
            }
          }
          MANIFEST

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: v${{ needs.meta.outputs.version }}
          name: Operon v${{ needs.meta.outputs.version }}
          body: |
            ## What's Changed
            ${{ steps.notes.outputs.notes }}

            ## Downloads
            | Platform | Download |
            |----------|----------|
            | Windows x64 | `operon-windows-x64-${{ needs.meta.outputs.version }}.zip` |
            | macOS ARM64 | `operon-macos-arm64-${{ needs.meta.outputs.version }}.tar.gz` |
            | macOS x64 | `operon-macos-x64-${{ needs.meta.outputs.version }}.tar.gz` |
            | Linux x64 | `operon-linux-x64-${{ needs.meta.outputs.version }}.tar.gz` |
            | Web | `operon-web.tar.gz` |
            | API Server | `operon-api-server-*` |
          prerelease: ${{ needs.meta.outputs.prerelease }}
          files: |
            artifacts/**/*.zip
            artifacts/**/*.tar.gz
            artifacts/**/update-manifest.json
          fail_on_unmatched_files: false
```

---

## Release Strategy

### Versioning Scheme

Follow [Semantic Versioning](https://semver.org/):

```
MAJOR.MINOR.PATCH[-PRERELEASE]
0.1.0        → initial development
0.2.0        → new features (non-breaking)
0.2.1        → bug fixes only
1.0.0        → first stable release
1.1.0-beta.1 → pre-release (marked as prerelease on GitHub)
```

### Release Flow

```
feature branch → PR → main (tests run) → tag v0.2.0 → release workflow → GitHub Release
```

**Step-by-step:**

1. **Develop** on feature branches, merge via PR to `main`
2. **Bump version** in `Cargo.toml` (see [Version Bumping](#version-bumping))
3. **Tag** the release commit:
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```
4. The `release.yml` workflow triggers automatically
5. Built artifacts are attached to a GitHub Release
6. Desktop clients check the update manifest on next launch

### Pre-releases

Tags with a hyphen (e.g. `v1.0.0-beta.1`) are automatically marked as
**prerelease** on GitHub. Pre-releases are not offered to stable-channel
users by the auto-updater.

### Hotfix Flow

```
main → hotfix/fix-crash → cherry-pick → tag v0.2.1 → release
```

---

## Version Bumping

### Manual (recommended for now)

Update the version in the root `Cargo.toml`:

```toml
[package]
name = "operon-dioxus"
version = "0.2.0"     # ← bump this
```

Then commit and tag:

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 0.2.0"
git tag v0.2.0
git push origin main v0.2.0
```

### Automated with `cargo-release` (optional)

```bash
cargo install cargo-release

# Dry run — shows what would change
cargo release patch --dry-run

# Real bump: updates Cargo.toml, commits, tags, pushes
cargo release patch --execute

# For minor / major:
cargo release minor --execute
cargo release major --execute
```

Add to root `Cargo.toml` (optional config):

```toml
[workspace.metadata.release]
shared-version = true
tag-name = "v{{version}}"
pre-release-commit-message = "chore: release v{{version}}"
```

### Justfile Recipe

Add to `Justfile`:

```just
# Create a release tag. Usage: just release 0.2.0
release version:
    @echo "Bumping to v{{version}}..."
    sed -i 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    cargo check  # verify it compiles
    git add Cargo.toml Cargo.lock
    git commit -m "chore: release v{{version}}"
    git tag "v{{version}}"
    git push origin main "v{{version}}"
    @echo "Release v{{version}} pushed — CI will build and publish."
```

---

## Desktop Auto-Updates

Since Operon uses Dioxus with Wry (not Tauri), there's no built-in updater.
Here's how to implement one.

### Architecture

```
┌──────────────────┐       HTTPS GET        ┌──────────────────────────┐
│   Operon Desktop │ ───────────────────────►│  GitHub Releases API     │
│   (on launch)    │                         │  /repos/.../releases     │
│                  │◄─────────────────────── │  → update-manifest.json  │
│                  │   { version, url, … }   └──────────────────────────┘
│                  │
│  Compare current │   if newer:
│  vs remote ver   │   ─── show prompt ──►  User clicks "Update"
│                  │                         │
│                  │   Download .zip/.tar.gz │
│                  │   Extract to temp dir   │
│                  │   Swap binary + restart │
└──────────────────┘
```

### Implementation — Update Checker Module

Add to the desktop app (e.g. `src/update_checker.rs`):

```rust
use serde::Deserialize;

#[derive(Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub notes: String,
    pub platforms: std::collections::HashMap<String, PlatformAsset>,
}

#[derive(Deserialize)]
pub struct PlatformAsset {
    pub url: String,
    pub signature: String,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const UPDATE_CHECK_URL: &str =
    "https://github.com/YOUR_ORG/operon/releases/latest/download/update-manifest.json";

/// Check if a newer version is available.
pub async fn check_for_update() -> Option<UpdateManifest> {
    let resp = reqwest::get(UPDATE_CHECK_URL).await.ok()?;
    let manifest: UpdateManifest = resp.json().await.ok()?;

    let current = semver::Version::parse(CURRENT_VERSION).ok()?;
    let remote = semver::Version::parse(&manifest.version).ok()?;

    if remote > current {
        Some(manifest)
    } else {
        None
    }
}

/// Get the platform key for the current OS + arch.
pub fn platform_key() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "windows-x64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "macos-arm64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "macos-x64";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "linux-x64";
}
```

### Dependencies to add

```toml
# In root Cargo.toml [target.'cfg(not(target_arch = "wasm32"))'.dependencies]
semver = "1"
# reqwest is already a dependency
```

### UI Component — Update Banner

```rust
#[component]
fn UpdateBanner() -> Element {
    let update = use_resource(|| async { check_for_update().await });

    match update() {
        Some(Some(manifest)) => rsx! {
            div {
                class: "update-banner",
                "Operon v{manifest.version} is available! "
                a {
                    href: manifest.platforms.get(platform_key())
                        .map(|p| p.url.as_str())
                        .unwrap_or("#"),
                    target: "_blank",
                    "Download now"
                }
            }
        },
        _ => rsx! {},
    }
}
```

### Self-Replacing Binary (Advanced)

For seamless in-app updates on Windows:

```rust
/// Download the update, extract, and swap the running binary.
pub async fn apply_update(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = reqwest::get(url).await?.bytes().await?;
    let current_exe = std::env::current_exe()?;
    let backup = current_exe.with_extension("exe.bak");
    let temp_dir = tempfile::tempdir()?;

    // Extract archive to temp
    // (use zip crate for .zip, flate2+tar for .tar.gz)

    // Rename current → backup, new → current
    std::fs::rename(&current_exe, &backup)?;
    std::fs::copy(temp_dir.path().join("operon-dioxus.exe"), &current_exe)?;

    // Restart
    std::process::Command::new(&current_exe).spawn()?;
    std::process::exit(0);
}
```

### Update Check Frequency

- Check on app launch (with a 5-second delay so the UI loads first)
- Cache the last check timestamp in SQLite (`app_settings` table)
- Don't check more than once per 24 hours
- Respect a user preference: "Check for updates: Automatically / Manually / Never"

---

## Web Deployment

### Static Hosting (GitHub Pages, Netlify, Vercel, S3)

The `build-web` job produces a static bundle at:
```
target/dx/operon-dioxus/release/web/public/
```

#### GitHub Pages

Add to `.github/workflows/release.yml` (inside the `publish-release` job or as a new job):

```yaml
  deploy-web:
    needs: [meta, build-web]
    runs-on: ubuntu-latest
    permissions:
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.deploy.outputs.page_url }}
    steps:
      - uses: actions/download-artifact@v4
        with:
          name: operon-web
          path: web-dist/

      - uses: actions/configure-pages@v4

      - uses: actions/upload-pages-artifact@v3
        with:
          path: web-dist/

      - id: deploy
        uses: actions/deploy-pages@v4
```

#### Netlify / Vercel

Point the deploy command at the web artifact:

```bash
# Netlify
netlify deploy --prod --dir=target/dx/operon-dioxus/release/web/public/

# Vercel
vercel --prod target/dx/operon-dioxus/release/web/public/
```

---

## API Server Deployment

### Docker

Create `Dockerfile.api-server`:

```dockerfile
FROM rust:1.95-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p operon-api-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/operon-api-server /usr/local/bin/
EXPOSE 7878
ENV OPN_BIND_ADDR=0.0.0.0:7878
CMD ["operon-api-server"]
```

### CI Docker Build + Push

```yaml
  deploy-api:
    needs: [meta]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - uses: docker/build-push-action@v5
        with:
          context: .
          file: Dockerfile.api-server
          push: true
          tags: |
            ghcr.io/${{ github.repository }}/api-server:${{ needs.meta.outputs.version }}
            ghcr.io/${{ github.repository }}/api-server:latest
```

### SSH Deploy (VPS)

```yaml
  deploy-api-ssh:
    needs: [meta, build-api-server]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          name: operon-api-server-linux-x64
          path: artifact/

      - name: Deploy to server
        uses: appleboy/scp-action@v0.1.7
        with:
          host: ${{ vars.DEPLOY_HOST }}
          username: ${{ vars.DEPLOY_USER }}
          key: ${{ secrets.DEPLOY_SSH_KEY }}
          source: "artifact/*.tar.gz"
          target: "/opt/operon/releases/${{ needs.meta.outputs.version }}"

      - name: Restart service
        uses: appleboy/ssh-action@v1.0.3
        with:
          host: ${{ vars.DEPLOY_HOST }}
          username: ${{ vars.DEPLOY_USER }}
          key: ${{ secrets.DEPLOY_SSH_KEY }}
          script: |
            cd /opt/operon/releases/${{ needs.meta.outputs.version }}
            tar xzf artifact/*.tar.gz
            sudo systemctl restart operon-api-server
```

---

## Branch Protection & Merge Rules

Configure in **Settings → Branches → Branch protection rules** for `main`:

| Rule | Value |
|------|-------|
| Require PR before merging | ✅ |
| Required approvals | 1 |
| Require status checks to pass | ✅ `unit-and-integration`, `wasm-tests`, `e2e` |
| Require branches to be up to date | ✅ |
| Require signed commits | Optional (recommended for releases) |
| Restrict who can push to matching branches | Team leads only |
| Allow force pushes | ❌ Never |
| Allow deletions | ❌ |

### Tag Protection

Under **Settings → Tags → Protected tags**, add pattern `v*` to prevent
unauthorized tag creation.

---

## Maintenance & Troubleshooting

### Common CI Issues

| Issue | Fix |
|-------|-----|
| `dx and dioxus versions are incompatible!` | Pin dx: `cargo install dioxus-cli --version 0.7.1 --locked` |
| Linux build fails: `webkit2gtk-4.1 not found` | Ensure `libwebkit2gtk-4.1-dev` is in apt install step |
| Windows build: `link.exe not found` | The `windows-latest` runner has MSVC pre-installed — this should not happen in CI |
| macOS notarization fails | Check `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID` secrets |
| `permission_bridge` Unix socket error on Windows | Already gated with `#[cfg(unix)]` — no action needed |
| Cache miss / slow builds | Check `Swatinem/rust-cache` key matches; consider separate cache keys per target |

### Monitoring Releases

- Enable **GitHub → Settings → Notifications → Releases** for the repo
- Use GitHub's **Release RSS feed**: `https://github.com/ORG/operon/releases.atom`
- Add a Slack/Discord webhook to the release workflow:

```yaml
      - name: Notify Slack
        if: success()
        uses: slackapi/slack-github-action@v1
        with:
          payload: |
            {
              "text": "🚀 Operon v${{ needs.meta.outputs.version }} released!\nhttps://github.com/${{ github.repository }}/releases/tag/v${{ needs.meta.outputs.version }}"
            }
        env:
          SLACK_WEBHOOK_URL: ${{ secrets.SLACK_WEBHOOK_URL }}
```

### Rollback

To roll back a bad release:

```bash
# Delete the release on GitHub (keeps the tag)
gh release delete v0.2.0 --yes

# Or delete both tag and release
gh release delete v0.2.0 --yes --cleanup-tag
git push origin :refs/tags/v0.2.0

# Point users at the previous version
gh release edit v0.1.0 --latest
```

### Cleaning Old Artifacts

GitHub retains workflow artifacts for 90 days by default. The workflow sets
`retention-days: 7` for build artifacts (release assets live permanently on
the GitHub Release). Adjust in **Settings → Actions → General → Artifact and
log retention**.

---

## Quick Reference — Release Checklist

```
□ All tests passing on main
□ Version bumped in Cargo.toml
□ CHANGELOG updated (instructions/changelog.md)
□ Commit: "chore: release v0.2.0"
□ Tag: git tag v0.2.0 && git push origin main v0.2.0
□ Verify release workflow completes (Actions tab)
□ Verify GitHub Release has all platform assets
□ Verify update-manifest.json is attached
□ Smoke-test download on Windows/macOS/Linux
□ Announce (Slack/Discord/email)
```
