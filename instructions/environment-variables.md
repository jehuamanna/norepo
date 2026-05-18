# Environment Variables

## Overview

Operon uses environment variables for runtime configuration, API keys, and server settings. Variables are organized by component.

---

## LLM API Keys

| Variable | Required | Description |
|---|---|---|
| `ANTHROPIC_API_KEY` | For Anthropic | Anthropic Claude API key (`sk-ant-...`) |
| `OPENAI_API_KEY` | For OpenAI | OpenAI API key (`sk-...`) |
| `GOOGLE_API_KEY` | For Gemini | Google Gemini API key (`AIza...`) |

**Lookup order**: SecretStore (OS keyring on desktop) → Environment variable → Error

**Security**: Never commit API keys. Use OS keyring (desktop) or SecretStore for production.

---

## API Server (`operon-api-server`)

| Variable | Default | Required | Description |
|---|---|---|---|
| `OPN_BIND_ADDR` | `127.0.0.1:7878` | No | Listen address and port |
| `OPN_DB_PATH` | `./operon.db` | No | SQLite database file path |
| `OPN_HOSTNAME` | `localhost` | No | Server hostname (for URLs in emails, etc.) |

---

## Agent Runtime Configuration

Configuration is loaded via **Figment** from `operon.toml` + environment variables with `OPERON_` prefix. Nested keys use double underscore `__`.

| Variable | Default | Description |
|---|---|---|
| `OPERON_RUNTIME__DEFAULT_MODEL` | `claude-sonnet-4-6` | Default LLM model |
| `OPERON_RUNTIME__LOG_FILTER` | `debug` | tracing filter directive |
| `OPERON_PROVIDERS__ANTHROPIC__API_URL` | `https://api.anthropic.com` | Anthropic API base URL |
| `OPERON_PROVIDERS__ANTHROPIC__MODEL` | `claude-sonnet-4-6` | Anthropic model name |
| `OPERON_PROVIDERS__ANTHROPIC__MAX_TOKENS` | `4096` | Max response tokens |
| `OPERON_PROVIDERS__ANTHROPIC__ANTHROPIC_VERSION` | `2024-06-15` | API version header |
| `OPERON_PROVIDERS__ANTHROPIC__ANTHROPIC_BETA` | (empty) | Beta features header |
| `OPERON_MEMORY__KIND` | `in_memory` | Memory store type (`in_memory` or `sqlite`) |
| `OPERON_MEMORY__SQLITE_PATH` | — | Path for SQLite memory store |

### Equivalent `operon.toml`

```toml
[runtime]
default_model = "claude-sonnet-4-6"
log_filter = "debug"

[providers.anthropic]
api_url = "https://api.anthropic.com"
model = "claude-sonnet-4-6"
max_tokens = 4096
anthropic_version = "2024-06-15"
anthropic_beta = ""

[memory]
kind = "in_memory"
sqlite_path = "/path/to/memory.db"

[[mcp.servers]]
name = "example"
transport = "stdio"
command = "/usr/bin/example-mcp"
args = ["arg1"]
```

---

## Testing & CI

| Variable | Default | Description |
|---|---|---|
| `OPERON_E2E_BASE_URL` | `http://localhost:8123` | Playwright test server URL |
| `OPERON_E2E_HEADED` | (unset) | Non-empty = headed Playwright mode |
| `CI` | (unset) | Present = CI mode (2 retries, 1 worker, forbid `.only`) |

---

## Build-Time Variables

| Variable | Description |
|---|---|
| `CARGO_FEATURE_DESKTOP` | Set when building with `--features desktop` |
| `CARGO_FEATURE_WEB` | Set when building with `--features web` |
| `CARGO_FEATURE_WASM_SQLITE` | Set when building with `--features wasm-sqlite` |

These are not manually set — Cargo manages them based on feature flags.

---

## Development vs Production

| Variable | Development | Production |
|---|---|---|
| `OPN_BIND_ADDR` | `127.0.0.1:7878` | `0.0.0.0:7878` (or behind reverse proxy) |
| `OPN_DB_PATH` | `./operon.db` | `/data/operon.db` (persistent volume) |
| `OPN_HOSTNAME` | `localhost` | `operon.example.com` |
| `OPERON_RUNTIME__LOG_FILTER` | `debug` | `info` or `warn` |
| API keys | Set in shell | OS keyring or secrets manager |

---

## Security Notes

- **Never commit** API keys or secrets to version control
- **Desktop**: Use OS keyring (macOS Keychain, Linux libsecret, Windows Credential Manager)
- **Server**: Use environment variables from secrets manager (AWS Secrets Manager, Vault, etc.)
- **CI**: Use CI secret variables (GitHub Actions secrets, GitLab CI variables)
- API key environment variables are the **fallback** — prefer SecretStore

---

## Setting Environment Variables

### Linux/macOS (shell)

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

### Windows (PowerShell)

```powershell
$env:ANTHROPIC_API_KEY = "sk-ant-..."
```

### Docker

```bash
docker run -e ANTHROPIC_API_KEY="sk-ant-..." operon-api
```

### Docker Compose

```yaml
environment:
  - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
```
