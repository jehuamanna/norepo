//! Cross-cutting UI primitives.
//!
//! Currently hosts the colourless-icon system: a single [`icon::Icon`] component renders
//! inline SVG using `currentColor` so the parent's CSS `color` (already theme-aware via
//! `--vscode-*` variables) tints the glyph automatically.

pub mod icon;

pub use icon::Icon;
