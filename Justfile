# Justfile — single source of truth for test/dev recipes in operon-dioxus.
#
# Install `just` if you don't have it:
#     cargo install just
#
# Required tools (the bootstrap recipe installs the project-side ones):
#     - rustup with `wasm32-unknown-unknown` target  (auto via rust-toolchain.toml)
#     - dx (dioxus-cli):      cargo install dioxus-cli --locked
#     - wasm-pack:            cargo install wasm-pack
#     - just:                 cargo install just
#     - node ≥ 20 + npm
#
# Authored under the "Playwright for testing" Archon seed
# (84185cbf-0b4f-4211-bb33-145a9817ac0c, Plans-Phase-1-test-toolchain-and-env).

default:
    @just --list

# One-time setup of project-side dependencies (idempotent).
bootstrap:
    cargo fetch
    npm ci
    npx playwright install --with-deps chromium

# Tier 1: pure-Rust unit tests (inline #[cfg(test)] mod tests blocks).
test-unit:
    cargo test --lib

# Tier 2: Cargo integration tests (tests/*.rs).
test-integration:
    cargo test --tests

# Sync the wasm-pack-cached chromedriver to match the local google-chrome
# version. Run once after the first wasm-pack invocation seeds the cache,
# and again whenever Chrome auto-updates and breaks the existing match.
sync-chromedriver:
    bash scripts/sync-chromedriver.sh

# Tier 3: browser-DOM integration tests run in headless Chromium.
# Requires `wasm-pack` and a `google-chrome` whose major version matches the
# wasm-pack-cached chromedriver. If you hit a `signal: 9 (SIGKILL)` driver
# crash on first run, `just sync-chromedriver` and retry.
test-wasm:
    wasm-pack test --headless --chrome tests-wasm

# Tier 4: Playwright e2e specs against `dx serve --platform web`.
test-e2e:
    npx playwright test

# Run all four tiers in order; abort on first failure.
test-all: test-unit test-integration test-wasm test-e2e

# Convenience: open the most recent Playwright HTML report.
e2e-report:
    npx playwright show-report

# Convenience: run e2e in headed/UI mode for debugging.
test-e2e-ui:
    npx playwright test --ui
