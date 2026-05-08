//! Lightweight YAML-frontmatter parser for skill notes.
//!
//! Skills look like:
//! ```text
//! ---
//! skill_name: ba-intake
//! skill_version: 3
//! ---
//!
//! You are a Business Analyst...
//! ```
//!
//! We parse a minimal subset (top-level `key: value` pairs, ignoring
//! nested objects / arrays for now) so the v1 view can pull `skill_name`
//! out of the frontmatter without depending on a full YAML crate.

/// Split a skill note's content into `(frontmatter_lines, body)`.
/// Returns `(None, full_content)` when the content doesn't begin with
/// a `---` fence — every other line stays in the body verbatim.
pub fn split(content: &str) -> (Option<Vec<&str>>, &str) {
    let trimmed_start = content.trim_start_matches('\u{feff}');
    let lookahead = trimmed_start.trim_start();
    if !lookahead.starts_with("---") {
        return (None, content);
    }
    let after_first_fence = match lookahead.split_once("---") {
        Some((_, rest)) => rest,
        None => return (None, content),
    };
    // Skip the immediate newline after `---`.
    let after_first_fence = after_first_fence.strip_prefix('\n').unwrap_or(after_first_fence);
    // Find the closing fence: a line that's exactly `---` (allowing CR).
    let mut frontmatter_end: Option<usize> = None;
    let mut offset = 0usize;
    for line in after_first_fence.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            frontmatter_end = Some(offset);
            // Body starts after this line's terminator.
            offset += line.len();
            break;
        }
        offset += line.len();
    }
    let Some(end) = frontmatter_end else {
        return (None, content);
    };
    let frontmatter_text = &after_first_fence[..end];
    let body_start = offset;
    // Body in the original `after_first_fence` slice — still includes
    // a potential leading newline; trim it.
    let body = after_first_fence[body_start..].trim_start_matches(['\n', '\r']);
    let lines: Vec<&str> = frontmatter_text.lines().collect();
    (Some(lines), body)
}

/// Pull a top-level `key: value` field from frontmatter lines. Trims
/// surrounding whitespace + outer quotes. Returns `None` if not found.
pub fn field<'a>(lines: &'a [&'a str], key: &str) -> Option<&'a str> {
    for line in lines {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                let v = v.trim();
                let v = v.trim_matches(|c| c == '"' || c == '\'');
                return Some(v);
            }
        }
    }
    None
}

/// Slug a string for use as a filename: lowercase, replace non-alnum with
/// `-`, collapse runs of dashes, trim leading/trailing dashes. Empty
/// inputs become "untitled".
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "untitled".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_returns_none_when_no_frontmatter() {
        let s = "no fence here\nlive body";
        let (fm, body) = split(s);
        assert!(fm.is_none());
        assert_eq!(body, s);
    }

    #[test]
    fn split_extracts_frontmatter_and_body() {
        let s = "---\nskill_name: ba-intake\nskill_version: 3\n---\n\nbody starts here";
        let (fm, body) = split(s);
        let fm = fm.expect("frontmatter present");
        assert_eq!(fm, vec!["skill_name: ba-intake", "skill_version: 3"]);
        assert_eq!(body, "body starts here");
    }

    #[test]
    fn split_handles_no_closing_fence() {
        let s = "---\nskill_name: foo\n(no close)";
        let (fm, body) = split(s);
        assert!(fm.is_none());
        assert_eq!(body, s);
    }

    #[test]
    fn field_pulls_top_level_key() {
        let lines = vec!["skill_name: ba-intake", "skill_version: 3"];
        assert_eq!(field(&lines, "skill_name"), Some("ba-intake"));
        assert_eq!(field(&lines, "skill_version"), Some("3"));
        assert_eq!(field(&lines, "missing"), None);
    }

    #[test]
    fn field_strips_quotes() {
        let lines = vec![r#"skill_name: "quoted slug""#];
        assert_eq!(field(&lines, "skill_name"), Some("quoted slug"));
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("BA Intake"), "ba-intake");
        assert_eq!(slugify("hello, world!"), "hello-world");
        assert_eq!(slugify("___"), "untitled");
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("a--b"), "a-b");
    }
}
