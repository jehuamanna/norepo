//! In-memory sample notes shown by the [`super::NotesExplorer`] panel.
//!
//! Tuple form: `(note_id, title, content)`. The third sample is the comprehensive
//! markdown-construct fixture used by the Phase 6 markdown plugin tests.

pub const SAMPLES: &[(&str, &str, &str)] = &[
    (
        "sample-readme",
        "README.md",
        "# Operon Shell\n\nA pluggable VS Code-style frame for Rust + Dioxus.",
    ),
    (
        "sample-todo",
        "TODO.md",
        "# Things to do\n\n- Build the shell\n- Test the plugin system\n- Ship the first plugin",
    ),
    ("sample-features", "Markdown Showcase", SHOWCASE),
];

const SHOWCASE: &str = r#"---
title: Markdown Showcase
---

# H1 Heading

## H2 Heading

### H3 Heading

A paragraph with **bold** and *emphasis* and `inline code` and a [link](https://dioxuslabs.com/).

> A block quote with **bold** inside.

- bullet
- bullet with `code`
- bullet

1. one
2. two

```rust
fn main() { println!("hi"); }
```

![header](/assets/header.svg)

---

End.
"#;
