//! In-memory sample notes shown by the [`super::NotesExplorer`] panel.
//!
//! Tuple form: `(note_id, title, content)`. The third sample becomes a comprehensive
//! markdown-construct fixture in Phase 6 once the markdown plugin lands.

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
    (
        "sample-features",
        "Markdown Showcase",
        "# Markdown Showcase\n\nReplaced with a richer fixture in Phase 6.",
    ),
];
