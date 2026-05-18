# Development

Your new bare-bones project includes minimal organization with a single `main.rs` file and a few assets.

```
project/
├─ assets/ # Any assets that are used by the app should be placed here
├─ src/
│  ├─ main.rs # main.rs is the entry point to your application and currently contains all components for the app
├─ Cargo.toml # The Cargo.toml file defines the dependencies and feature flags for your project
```

### Automatic Tailwind (Dioxus 0.7+)

As of Dioxus 0.7, there no longer is a need to manually install tailwind. Simply `dx serve` and you're good to go!

Automatic tailwind is supported by checking for a file called `tailwind.css` in your app's manifest directory (next to Cargo.toml). To customize the file, use the dioxus.toml:

```toml
[application]
tailwind_input = "my.css"
tailwind_output = "assets/out.css" # also customize the location of the out file!
```

### Tailwind Manual Install

To use tailwind plugins or manually customize tailwind, you can can install the Tailwind CLI and use it directly.

### Tailwind
1. Install npm: https://docs.npmjs.com/downloading-and-installing-node-js-and-npm
2. Install the Tailwind CSS CLI: https://tailwindcss.com/docs/installation/tailwind-cli
3. Run the following command in the root of the project to start the Tailwind CSS compiler:

```bash
npx @tailwindcss/cli -i ./input.css -o ./assets/tailwind.css --watch
```

### Serving Your App

Run the following command in the root of your project to start developing with the default platform:

```bash
dx serve
```

To run for a different platform, use the `--platform platform` flag. E.g.
```bash
dx serve --platform desktop
```

# Testing

The project ships a four-tier testing stack. The full TDD skill (tiers, conventions, walkthroughs, anti-patterns) lives in the Archon note **Test Case Specs** (`7094db6c-00d6-41d1-bc04-8b91cce36a5b`). Every test under this repo MUST follow it.

| Tier | Lives in | Run command |
|---|---|---|
| Unit (Rust, inline) | `src/**/mod.rs` `#[cfg(test)] mod tests` | `just test-unit` |
| Integration (Rust) | `tests/*.rs` | `just test-integration` |
| Browser-DOM | `tests-wasm/tests/*.rs` (`wasm-bindgen-test`) | `just test-wasm` |
| End-to-end | `e2e/specs/*.spec.ts` (Playwright) | `just test-e2e` |

Run them all in order with `just test-all`.

## One-time setup

Install the developer tools that the recipes depend on:

```bash
# rust toolchain pin: rust-toolchain.toml will auto-install stable + wasm32 target
cargo install just dioxus-cli wasm-pack --locked
# pin node version (uses .nvmrc)
nvm use
# install project dependencies (cargo fetch + npm ci + playwright browsers)
just bootstrap
```

## Running tests

```bash
just test-unit          # < 5 s
just test-integration   # < 30 s
just test-wasm          # < 60 s, headless Chromium
just test-e2e           # < 4 min, Chromium against `dx serve --platform web`
just test-all           # all four, fail-fast
```

Set `OPERON_E2E_BASE_URL` to point Playwright at an already-running dev
server (skips the embedded `dx serve` boot).


## License

Operon is licensed under the [Apache License, Version 2.0](LICENSE).

Copyright 2026 Jehu Amanna and contributors. See [NOTICE](NOTICE) for attribution requirements.
