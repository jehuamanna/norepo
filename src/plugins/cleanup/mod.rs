//! Cross-plugin cleanup hooks that run on note deletion.
//!
//! Note deletion in operon-dioxus has historically been a per-call-site
//! affair: the explorer wiped blobs, [`super::artifact::relocate`] wiped
//! artifact dirs on the way through the wrapper, and skill materializations
//! at `<repo>/.claude/skills/<slug>.md` were left as orphans. This module
//! consolidates those into a single async helper so every call site does
//! the same thing.

pub mod note_delete;
pub mod trash;
