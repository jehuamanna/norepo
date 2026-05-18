# Service Documentation Template

## {Service Name}

**Location**: `crates/{crate}/` or `src/{module}/`

**Type**: Background Service / API Service / Plugin / Integration

---

## Overview

Describe the service's purpose, lifecycle, and how it integrates with the rest of the system.

---

## Architecture

```mermaid
graph LR
    INPUT[Input Source] --> SERVICE[{Service Name}]
    SERVICE --> OUTPUT[Output Destination]
    SERVICE --> DEPS[Dependencies]
```

---

## Configuration

| Key | Type | Default | Description |
|---|---|---|---|
| `config_key` | `string` | `default` | What it controls |

**Environment Variables**:

| Variable | Default | Description |
|---|---|---|
| `ENV_VAR` | `value` | What it controls |

---

## Lifecycle

### Initialization

```
1. Service created with configuration
2. Dependencies resolved
3. Connections established
4. Ready to process requests
```

### Operation

```
1. Receive input (event/request/message)
2. Process according to business logic
3. Emit output (response/event/side-effect)
4. Update internal state
```

### Shutdown

```
1. Stop accepting new work
2. Drain in-progress operations
3. Close connections
4. Release resources
```

---

## API

### Input

| Input | Type | Source | Description |
|---|---|---|---|
| `input_name` | `Type` | Where it comes from | What it contains |

### Output

| Output | Type | Destination | Description |
|---|---|---|---|
| `output_name` | `Type` | Where it goes | What it contains |

---

## Error Handling

| Error | Cause | Recovery |
|---|---|---|
| `ErrorType` | When this happens | What the service does |

---

## Monitoring

### Metrics

| Metric | Type | Description |
|---|---|---|
| `metric_name` | counter/gauge/histogram | What it measures |

### Logging

```
tracing::info!("service event: {}", detail);
tracing::error!("service failure: {}", err);
```

---

## Dependencies

| Dependency | Type | Purpose |
|---|---|---|
| `dep_name` | Crate / Service / External | Why it's needed |

---

## Testing

**Test location**: `tests/{file}.rs`

```bash
cargo test -p {crate}
```

### Mocking

| Mock | Replaces | Usage |
|---|---|---|
| `MockService` | `RealService` | Integration tests |

---

## Related Documentation

- [Architecture](../architecture.md) — system context
- [How It Works](../how-it-works.md) — workflow details
- [Deployment](../deployment-guide.md) — production configuration
