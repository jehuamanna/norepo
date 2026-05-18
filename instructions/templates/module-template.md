# Module Documentation Template

## {Module Name}

**Location**: `crates/{crate-name}/` or `src/{module}/`

**Purpose**: Brief one-line description.

---

## Overview

Explain what this module does, why it exists, and how it fits into the architecture.

---

## Dependencies

| Dependency | Purpose |
|---|---|
| `crate-name` | Why it's used |

---

## Public API

### Structs

#### `StructName`

```rust
pub struct StructName {
    pub field: Type,
}
```

**Description**: What it represents.

### Traits

#### `TraitName`

```rust
pub trait TraitName {
    fn method(&self) -> ReturnType;
}
```

**Description**: What implementations must provide.

### Functions

#### `function_name()`

```rust
pub fn function_name(param: Type) -> Result<Output, Error>
```

**Description**: What it does, when to use it.

---

## Configuration

| Config Key | Default | Description |
|---|---|---|
| `KEY_NAME` | `default` | What it controls |

---

## Usage Example

```rust
use crate_name::StructName;

let instance = StructName::new();
instance.do_something();
```

---

## Error Handling

| Error | Cause | Resolution |
|---|---|---|
| `ErrorVariant` | When this happens | How to fix |

---

## Testing

**Test location**: `tests/{test_file}.rs` or inline `#[cfg(test)]`

```bash
cargo test -p {crate-name}
```

---

## Related Documentation

- [Architecture](../architecture.md) — system design context
- [How It Works](../how-it-works.md) — workflow details
