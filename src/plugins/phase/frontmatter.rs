//! Parser + writer for phase-note frontmatter.
//!
//! Reuses the shared `skill::frontmatter::{split, field}` helpers so
//! the parse rules match the rest of the codebase (no full YAML
//! crate; small known field set). Bodies without a frontmatter block
//! return defaults — phases tolerate user-authored notes that haven't
//! been touched by the scaffolding command yet.

use crate::plugins::skill::frontmatter::{field, split};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseFrontmatter {
    /// Primary sort key. Lower numbers render higher in the explorer's
    /// phase list. `None` means "unset" — sort to the tail.
    pub order: Option<i32>,
    /// Free-form display name (e.g. "Discovery", "Multiplayer MVP").
    /// Falls back to `LocalNote.title` when missing.
    pub label: Option<String>,
}

pub fn parse(body: &str) -> PhaseFrontmatter {
    let (lines_opt, _) = split(body);
    let lines = match lines_opt {
        Some(l) => l,
        None => return PhaseFrontmatter::default(),
    };
    let order = field(&lines, "phase_order").and_then(|s| s.parse::<i32>().ok());
    let label = field(&lines, "phase_label").map(str::to_string);
    PhaseFrontmatter { order, label }
}

/// Render `fm` plus the existing body (everything after the existing
/// frontmatter, if any) back into a string. Used by the scaffolding
/// command to write a freshly-named phase note.
pub fn serialize(fm: &PhaseFrontmatter, body_after_frontmatter: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    if let Some(order) = fm.order {
        out.push_str(&format!("phase_order: {order}\n"));
    }
    if let Some(label) = &fm.label {
        // Wrap in double-quotes so multi-word labels survive the
        // line-based parser's `trim_matches('"')`. The shared `field`
        // helper doesn't decode backslash escapes, so we drop any
        // embedded `"` rather than emit an unparseable wrapped value
        // — phase labels are user-typed display names where quotes
        // would be unusual.
        let safe = label.replace('"', "");
        out.push_str(&format!("phase_label: \"{safe}\"\n"));
    }
    out.push_str("---\n");
    out.push_str(body_after_frontmatter);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_body_returns_default() {
        let fm = parse("");
        assert!(fm.order.is_none());
        assert!(fm.label.is_none());
    }

    #[test]
    fn parse_extracts_order_and_label() {
        let body = "---\nphase_order: 2\nphase_label: \"Multiplayer MVP\"\n---\nbody here\n";
        let fm = parse(body);
        assert_eq!(fm.order, Some(2));
        assert_eq!(fm.label.as_deref(), Some("Multiplayer MVP"));
    }

    #[test]
    fn parse_handles_missing_label() {
        let body = "---\nphase_order: 0\n---\n";
        let fm = parse(body);
        assert_eq!(fm.order, Some(0));
        assert!(fm.label.is_none());
    }

    #[test]
    fn parse_handles_missing_order() {
        let body = "---\nphase_label: Discovery\n---\n";
        let fm = parse(body);
        assert!(fm.order.is_none());
        assert_eq!(fm.label.as_deref(), Some("Discovery"));
    }

    #[test]
    fn serialize_round_trips_multi_word_label() {
        let fm = PhaseFrontmatter {
            order: Some(1),
            label: Some("Multiplayer MVP".into()),
        };
        let serialized = serialize(&fm, "body content\n");
        let reparsed = parse(&serialized);
        assert_eq!(reparsed.order, Some(1));
        assert_eq!(reparsed.label.as_deref(), Some("Multiplayer MVP"));
        assert!(serialized.ends_with("body content\n"));
    }

    #[test]
    fn serialize_strips_embedded_quotes_from_label() {
        // Inner quotes can't be encoded without the shared `field`
        // parser supporting escapes — strip them in serialize so we
        // never emit unparseable frontmatter.
        let fm = PhaseFrontmatter {
            order: None,
            label: Some("Phase \"One\"".into()),
        };
        let out = serialize(&fm, "");
        let reparsed = parse(&out);
        assert_eq!(reparsed.label.as_deref(), Some("Phase One"));
    }

    #[test]
    fn serialize_omits_unset_fields() {
        let fm = PhaseFrontmatter::default();
        let out = serialize(&fm, "");
        assert!(!out.contains("phase_order"));
        assert!(!out.contains("phase_label"));
    }
}
