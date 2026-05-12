//! Parse `<details><summary>Revision N (YYYY-MM-DD)</summary>…</details>`
//! blocks out of artifact bodies and expose them as a flat list the
//! Revision dropdown can render.
//!
//! The seed-skills format stashes prior revisions inside collapsed
//! `<details>` blocks at the bottom of the body whenever a skill
//! re-runs on a previously-produced artifact (see `runner.rs:225`+ and
//! the per-skill "Revision behavior (re-runs)" sections in
//! `seed-skills-updated/`). The dropdown lets the user pick which
//! revision to view; selecting "Current" returns the editable head
//! body (everything outside any `<details>` block), selecting a prior
//! revision shows that block's inner content read-only.
//!
//! This parser is intentionally string-based — `<details>` is the
//! literal HTML tag the skill emits, not a Markdown construct, so
//! pulldown-cmark won't help. We scan for `<details>` openings, find
//! the matching `</details>` close, read the `<summary>…</summary>`
//! header, and slice out the inner body. Tolerant of whitespace +
//! casing variations; nested `<details>` blocks (the seed skills don't
//! emit them today, but a hand-edited artifact might) are handled by
//! matching balanced opens/closes.

#![cfg(not(target_arch = "wasm32"))]

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRevision {
    /// Display label. `"Current"` for the head body; the literal text
    /// of the `<summary>` element (e.g. `"Revision 2 (2026-05-12)"`)
    /// for stashed revisions.
    pub label: String,
    /// Body markdown content for this revision. Read-only when shown;
    /// see `selected_revision` in the artifact view.
    pub body: String,
}

/// Top-level entry: return the head body as `Current` plus every
/// `<details>` block found. Order: Current first, then revisions in
/// the order they appear in the source (which the seed-skills
/// convention keeps newest-first — most-recently-stashed at the top
/// of the `<details>` stack).
pub fn parse_revisions(body: &str) -> Vec<ArtifactRevision> {
    let mut out: Vec<ArtifactRevision> = Vec::new();
    let mut current = String::new();
    let mut revisions: Vec<ArtifactRevision> = Vec::new();

    let bytes = body.as_bytes();
    let mut i = 0usize;
    let mut last_emit = 0usize;
    while i < bytes.len() {
        if !starts_with_ignore_ws(bytes, i, b"<details") {
            i += 1;
            continue;
        }
        // Flush content between last block and this `<details>` into
        // the Current body. Trim a trailing newline so consecutive
        // `<details>` blocks don't accumulate blank lines.
        current.push_str(&body[last_emit..i]);

        // Walk forward to the matching closing `</details>`, counting
        // nested opens so a hand-edited revision containing its own
        // `<details>` doesn't end the outer block prematurely.
        let block_end = find_balanced_details_end(bytes, i);
        let block_end = match block_end {
            Some(e) => e,
            None => {
                // Unbalanced `<details>` — give up and treat the rest
                // as Current rather than panic. Keeps malformed bodies
                // viewable.
                current.push_str(&body[i..]);
                last_emit = bytes.len();
                break;
            }
        };

        let block = &body[i..block_end];
        if let Some(rev) = parse_one_block(block) {
            revisions.push(rev);
        }

        i = block_end;
        last_emit = block_end;
    }
    current.push_str(&body[last_emit..]);

    out.push(ArtifactRevision {
        label: "Current".to_string(),
        body: trim_trailing_blank_lines(&current),
    });
    out.extend(revisions);
    out
}

fn starts_with_ignore_ws(bytes: &[u8], i: usize, prefix: &[u8]) -> bool {
    if i + prefix.len() > bytes.len() {
        return false;
    }
    bytes[i..i + prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Returns the byte index just AFTER the matching `</details>` close
/// tag, or `None` if no balanced close exists from `start`. Counts
/// nested `<details>` openings so a revision containing a hand-edited
/// inner `<details>` is preserved verbatim.
fn find_balanced_details_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut i = start;
    while i < bytes.len() {
        if starts_with_ignore_ws(bytes, i, b"<details") {
            depth += 1;
            // Skip past the opening tag so a malformed `<details<details>`
            // doesn't double-count.
            i = match find_tag_close(bytes, i) {
                Some(e) => e,
                None => return None,
            };
            continue;
        }
        if starts_with_ignore_ws(bytes, i, b"</details>") {
            depth -= 1;
            let end = i + b"</details>".len();
            if depth == 0 {
                return Some(end);
            }
            i = end;
            continue;
        }
        i += 1;
    }
    None
}

/// Skip past `<tag …>` — returns the index just after the `>`.
fn find_tag_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// Parse one full `<details>…</details>` block into an
/// `ArtifactRevision`. Extracts `<summary>` text as the label and
/// everything between the summary close and the details close as
/// the body. Returns `None` if `<summary>` is missing — a malformed
/// block doesn't become a revision option.
fn parse_one_block(block: &str) -> Option<ArtifactRevision> {
    let bytes = block.as_bytes();
    let summary_open = find_ci(bytes, 0, b"<summary")?;
    let summary_text_start = find_tag_close(bytes, summary_open)?;
    let summary_close = find_ci(bytes, summary_text_start, b"</summary>")?;
    let label_raw = &block[summary_text_start..summary_close];
    let label = label_raw.trim().to_string();
    if label.is_empty() {
        return None;
    }

    let body_start = summary_close + b"</summary>".len();
    let details_close = find_ci(bytes, body_start, b"</details>")?;
    let body_raw = &block[body_start..details_close];
    Some(ArtifactRevision {
        label,
        body: trim_trailing_blank_lines(body_raw.trim_start_matches('\n')),
    })
}

fn find_ci(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + needle.len() <= bytes.len() {
        if bytes[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn trim_trailing_blank_lines(s: &str) -> String {
    let mut out = s.to_string();
    while out.ends_with("\n\n") {
        out.pop();
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_details_returns_only_current() {
        let body = "# Feature: foo\n\n## Outcome\nSomething.";
        let revs = parse_revisions(body);
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].label, "Current");
        assert!(revs[0].body.contains("Outcome"));
    }

    #[test]
    fn one_details_block_yields_current_plus_revision() {
        let body = r#"# Feature: foo (rev 2)

## Outcome
Updated outcome.

<details>
<summary>Revision 1 (2026-05-09)</summary>

# Feature: foo

## Outcome
Original outcome.

</details>
"#;
        let revs = parse_revisions(body);
        assert_eq!(revs.len(), 2);
        assert_eq!(revs[0].label, "Current");
        assert!(revs[0].body.contains("Updated outcome"));
        assert!(!revs[0].body.contains("Original outcome"));
        assert_eq!(revs[1].label, "Revision 1 (2026-05-09)");
        assert!(revs[1].body.contains("Original outcome"));
    }

    #[test]
    fn multiple_revisions_preserve_order() {
        let body = r#"# Head body

<details><summary>Revision 3 (2026-05-12)</summary>
Body 3
</details>
<details><summary>Revision 2 (2026-05-10)</summary>
Body 2
</details>
<details><summary>Revision 1 (2026-05-09)</summary>
Body 1
</details>"#;
        let revs = parse_revisions(body);
        let labels: Vec<&str> = revs.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Current",
                "Revision 3 (2026-05-12)",
                "Revision 2 (2026-05-10)",
                "Revision 1 (2026-05-09)",
            ]
        );
        assert!(revs[1].body.contains("Body 3"));
        assert!(revs[2].body.contains("Body 2"));
        assert!(revs[3].body.contains("Body 1"));
    }

    #[test]
    fn current_body_excludes_every_details_block() {
        let body = r#"# Head

Head paragraph.

<details><summary>Revision 1 (2026-05-09)</summary>
Should not appear in Current.
</details>

After block."#;
        let revs = parse_revisions(body);
        assert_eq!(revs[0].label, "Current");
        let current = &revs[0].body;
        assert!(current.contains("Head paragraph."));
        assert!(current.contains("After block."));
        assert!(!current.contains("Should not appear"));
    }

    #[test]
    fn malformed_unclosed_details_falls_back_to_current() {
        // No closing `</details>` — bail-out path treats remainder
        // as Current rather than panicking.
        let body = "# Head\n\n<details><summary>Revision 1</summary>\nopen forever";
        let revs = parse_revisions(body);
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].label, "Current");
    }

    #[test]
    fn missing_summary_skips_the_block() {
        // `<details>` with no `<summary>` — block isn't a revision.
        let body = "# Head\n\n<details>\nWithout summary.\n</details>";
        let revs = parse_revisions(body);
        // Current still parsed (body before/after the bogus block);
        // the bogus block contributes no revision entry.
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].label, "Current");
    }

    #[test]
    fn case_insensitive_tag_match() {
        let body = r#"# Head

<DETAILS><Summary>Revision 1 (2026-05-09)</summary>
Body 1
</DETAILS>"#;
        let revs = parse_revisions(body);
        assert_eq!(revs.len(), 2);
        assert_eq!(revs[1].label, "Revision 1 (2026-05-09)");
        assert!(revs[1].body.contains("Body 1"));
    }
}
