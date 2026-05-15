//! Compute a unified diff for the *upcoming* file change in an Edit or
//! Write tool call, so the permission card can render it before the
//! user clicks Allow.
//!
//! This is read-only: the on-disk file is loaded for the "before"
//! snapshot but never written to. The "after" snapshot is computed by
//! applying the tool input the same way claude would — for `Edit` we
//! splice `old_string` → `new_string`; for `Write` we use `content`
//! verbatim.
//!
//! Output is a single string with `-`/`+`/` ` prefixes per line so the
//! card can render it in a `<pre>` block (Phase 1) and later swap in a
//! line-by-line component with red/green highlighting (follow-up).

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use serde_json::Value;
use similar::{ChangeTag, TextDiff};

/// Maximum diff size we render inline on the card. Larger diffs are
/// truncated with a "(...N more lines hidden)" footer. Keeps the card
/// from blowing up when claude rewrites a 10k-line file.
const MAX_DIFF_LINES: usize = 400;

/// Tool inputs we know how to diff. Anything outside this set returns
/// `None` and the card falls back to the raw JSON view.
pub enum DiffSource<'a> {
    /// `Edit` tool input: { file_path, old_string, new_string }.
    Edit {
        file_path: &'a Path,
        old_string: &'a str,
        new_string: &'a str,
    },
    /// `Write` tool input: { file_path, content }.
    Write {
        file_path: &'a Path,
        content: &'a str,
    },
}

/// Extract a `DiffSource` from a tool name + raw input. Returns `None`
/// when the tool isn't an editor or the input shape doesn't match.
pub fn diff_source_from<'a>(tool_name: &str, input: &'a Value) -> Option<DiffSource<'a>> {
    match tool_name {
        "Edit" => {
            let file_path = input.get("file_path")?.as_str()?;
            let old_string = input.get("old_string")?.as_str()?;
            let new_string = input.get("new_string")?.as_str()?;
            Some(DiffSource::Edit {
                file_path: Path::new(file_path),
                old_string,
                new_string,
            })
        }
        "Write" => {
            let file_path = input.get("file_path")?.as_str()?;
            let content = input.get("content")?.as_str()?;
            Some(DiffSource::Write {
                file_path: Path::new(file_path),
                content,
            })
        }
        _ => None,
    }
}

/// Render the upcoming change as a unified-diff string. Returns `None`
/// when the on-disk read fails *and* there's no synthetic "(new file)"
/// path to fall back on — callers can then render the raw JSON instead.
pub fn render(source: DiffSource<'_>) -> Option<String> {
    let (before, after, label) = match source {
        DiffSource::Edit {
            file_path,
            old_string,
            new_string,
        } => {
            let current = std::fs::read_to_string(file_path).ok()?;
            let after = current.replacen(old_string, new_string, 1);
            (current, after, file_path.display().to_string())
        }
        DiffSource::Write { file_path, content } => {
            // Write tolerates the file not existing — treat as "(new file)".
            let current =
                std::fs::read_to_string(file_path).unwrap_or_else(|_| String::new());
            let label = if current.is_empty() {
                format!("{} (new file)", file_path.display())
            } else {
                file_path.display().to_string()
            };
            (current, content.to_string(), label)
        }
    };
    Some(format_unified(&label, &before, &after))
}

fn format_unified(label: &str, before: &str, after: &str) -> String {
    if before == after {
        return format!("--- {label}\n(no textual change)\n");
    }
    let diff = TextDiff::from_lines(before, after);
    let mut out = String::new();
    out.push_str(&format!("--- {label}\n+++ {label}\n"));
    let mut emitted = 0usize;
    for change in diff.iter_all_changes() {
        if emitted >= MAX_DIFF_LINES {
            out.push_str("(\u{2026} more lines hidden \u{2026})\n");
            break;
        }
        let sign = match change.tag() {
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
            ChangeTag::Equal => ' ',
        };
        out.push(sign);
        // `change.value()` already includes its trailing newline (when
        // the source had one) — only append one ourselves when the
        // input didn't end the line so the rendered diff stays
        // line-perfect.
        let v = change.value();
        out.push_str(v);
        if !v.ends_with('\n') {
            out.push('\n');
        }
        emitted += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn edit_diff_shows_minus_plus() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "let x = 1;").unwrap();
        let input = json!({
            "file_path": f.path().to_str().unwrap(),
            "old_string": "let x = 1;",
            "new_string": "let x = 2;",
        });
        let src = diff_source_from("Edit", &input).unwrap();
        let diff = render(src).unwrap();
        assert!(diff.contains("-let x = 1;"), "diff missing minus line: {diff}");
        assert!(diff.contains("+let x = 2;"), "diff missing plus line: {diff}");
    }

    #[test]
    fn write_diff_handles_new_file() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("brand-new.txt");
        let input = json!({
            "file_path": path.to_str().unwrap(),
            "content": "hello\nworld\n",
        });
        let src = diff_source_from("Write", &input).unwrap();
        let diff = render(src).unwrap();
        assert!(diff.contains("(new file)"), "label missing new-file hint: {diff}");
        assert!(diff.contains("+hello"));
        assert!(diff.contains("+world"));
    }

    #[test]
    fn write_diff_against_existing_file() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "old line\n").unwrap();
        let input = json!({
            "file_path": f.path().to_str().unwrap(),
            "content": "new line\n",
        });
        let src = diff_source_from("Write", &input).unwrap();
        let diff = render(src).unwrap();
        assert!(diff.contains("-old line"));
        assert!(diff.contains("+new line"));
    }

    #[test]
    fn non_editor_tool_returns_none() {
        let input = json!({ "command": "ls" });
        assert!(diff_source_from("Bash", &input).is_none());
        assert!(diff_source_from("Read", &input).is_none());
    }

    #[test]
    fn malformed_edit_input_returns_none() {
        let input = json!({ "file_path": "/tmp/x.rs" }); // missing strings
        assert!(diff_source_from("Edit", &input).is_none());
    }

    #[test]
    fn identical_before_after_says_no_change() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "same\n").unwrap();
        let input = json!({
            "file_path": f.path().to_str().unwrap(),
            "content": "same\n",
        });
        let src = diff_source_from("Write", &input).unwrap();
        let diff = render(src).unwrap();
        assert!(diff.contains("no textual change"), "diff: {diff}");
    }
}
