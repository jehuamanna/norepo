//! Phase notes: project-root containers that group one batch of
//! requirements + epics together (Discovery, Phase 1, Multiplayer MVP,
//! …). Three-tier SDLC restructure, Phase B.
//!
//! A phase note has `NoteKind::Phase` (migration 020) and carries its
//! metadata in YAML frontmatter:
//!
//! ```yaml
//! ---
//! phase_order: 0
//! phase_label: "Discovery"
//! ---
//! ```
//!
//! `phase_order` is the primary sort key for the explorer's phase
//! listing; `phase_label` is the free-form human name and falls back
//! to the note's `title` column when absent.

pub mod frontmatter;

pub use frontmatter::{parse, serialize, PhaseFrontmatter};
