//! In-body revision history table for artifact notes.
//!
//! Format the author wants is a literal markdown table at the bottom
//! of the body, just above any cascade-stashed `<details>` blocks:
//!
//! ```markdown
//! ## Revision history
//!
//! | Revision | Date       | Derived from | Summary       |
//! |----------|------------|--------------|---------------|
//! | 1        | 2026-05-11 | manual       | Initial draft.|
//! ```
//!
//! This module is the only place that knows the on-disk shape. Anybody
//! appending a row goes through [`append_revision_row`]; anybody asking
//! "has this been Done-saved yet" goes through [`body_has_table`].
//!
//! Intentionally string-based — pulldown-cmark would parse the table
//! correctly but rewriting the body via markdown-AST would normalise
//! every other markdown construct around it. Surgical text edits keep
//! everything else byte-identical.

#![cfg(not(target_arch = "wasm32"))]

pub const REVISION_TABLE_HEADING: &str = "## Revision history";
const TABLE_COLUMNS_HEADER: &str = "| Revision | Date       | Derived from | Summary       |";
const TABLE_COLUMNS_SEPARATOR: &str =
    "|----------|------------|--------------|---------------|";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionTable {
    pub rows: Vec<RevisionRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionRow {
    /// 1-based monotonic counter. `append_revision_row` enforces this
    /// (calls `next_revision_number` internally); construct directly
    /// only when you've already done the math.
    pub revision: i64,
    /// `YYYY-MM-DD`, UTC. Use [`format_revision_date`] to build from
    /// a unix-ms timestamp.
    pub date: String,
    /// `"manual"` | `"claude"` | `"<skill-slug>"`. Free-form;
    /// pipe characters get escaped on write.
    pub derived_from: String,
    /// Single-line summary. Pipe and newline characters are escaped
    /// on write so the table stays parseable.
    pub summary: String,
}

/// `true` iff the body contains a `## Revision history` H2 heading at
/// the start of a line followed by a parseable table. Used by the
/// editor's default-mode dispatch to pick View (table present) vs
/// Edit (table absent) for fresh artifacts.
pub fn body_has_table(body: &str) -> bool {
    parse_revision_table(body).is_some()
}

/// Render a short, human-readable summary of what changed between
/// `prior` and `new`. Used by the filesystem watcher when an artifact
/// is overwritten externally (Claude `Write`/`Edit`, cascade output)
/// so the auto-appended `claude` row carries a meaningful description.
///
/// Manual revisions skip this — the user types their own summary
/// directly in the Done input box. Lives here (rather than in the
/// persistence layer) because the table is the only consumer.
pub fn compute_summary(prior: Option<&str>, new: Option<&str>) -> String {
    let Some(new) = new else {
        return "(non-UTF-8 body — change not summarised)".to_string();
    };
    let new_lines = new.lines().count();
    let Some(prior) = prior else {
        return format!("Initial save ({new_lines} lines)");
    };
    let prior_lines = prior.lines().count();
    let delta = new_lines as i64 - prior_lines as i64;
    if delta > 0 {
        format!("Added {delta} line(s) ({prior_lines} → {new_lines})")
    } else if delta < 0 {
        format!("Removed {} line(s) ({prior_lines} → {new_lines})", -delta)
    } else {
        format!("Edited body ({new_lines} lines)")
    }
}

/// Render a unix-ms timestamp as `YYYY-MM-DD` (UTC). Shares its
/// civil-from-days helper with the timestamp formatter in
/// `src/local_mode/editor/mod.rs`; we re-derive here so this module
/// doesn't depend on the editor.
pub fn format_revision_date(unix_ms: i64) -> String {
    if unix_ms < 0 || unix_ms > 253_402_300_799_999 {
        return format!("@{unix_ms}ms");
    }
    let days = (unix_ms / 1000).div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + (era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Find and parse the `## Revision history` section. Returns `None`
/// when the heading is missing or the table that follows is malformed.
///
/// Tolerant of: blank lines between heading and table, trailing
/// whitespace on rows, escaped pipes (`\|`). Strict about: heading
/// must appear at the start of a line, must be exactly `## ` (no `### `,
/// no inline-code wrapped). Two columns missing or mis-ordered → `None`.
pub fn parse_revision_table(body: &str) -> Option<RevisionTable> {
    let heading_pos = find_heading_at_line_start(body, REVISION_TABLE_HEADING)?;
    let after_heading = &body[heading_pos + REVISION_TABLE_HEADING.len()..];

    // Skip blank lines after the heading.
    let mut lines = after_heading.lines();
    let mut header_line: Option<&str> = None;
    for line in lines.by_ref() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        header_line = Some(t);
        break;
    }
    let header = header_line?;
    if !is_table_header(header) {
        return None;
    }
    // The next non-blank line must be the separator (`|---|---|...`).
    let sep = lines.next()?.trim();
    if !is_table_separator(sep) {
        return None;
    }

    let mut rows: Vec<RevisionRow> = Vec::new();
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            // A blank line terminates the table.
            break;
        }
        if !t.starts_with('|') {
            // Table ends as soon as we exit pipe-prefixed rows.
            break;
        }
        if let Some(row) = parse_data_row(t) {
            rows.push(row);
        }
        // Malformed individual rows are skipped silently rather than
        // failing the whole parse — keeps a hand-edited body usable.
    }

    Some(RevisionTable { rows })
}

/// Highest `revision` in the body's table + 1; `1` when no table or
/// no rows exist. Use this when constructing a `RevisionRow` so a
/// hand-edited table with gaps (1, 2, 5) still gets `6` next.
pub fn next_revision_number(body: &str) -> i64 {
    parse_revision_table(body)
        .map(|t| t.rows.iter().map(|r| r.revision).max().unwrap_or(0) + 1)
        .unwrap_or(1)
}

/// Remove the `## Revision history` section (heading + table) from
/// `body`, returning the body without it. No-op when the section is
/// absent. Used when materializing a skill body to disk: the
/// revision table is editor-only metadata and would otherwise leak
/// into Claude's prompt when it loads `.claude/skills/<slug>.md`.
///
/// Any sequence of 3+ newlines created at the cut point gets
/// collapsed to 2 (one blank line) so the surrounding paragraphs
/// stay separated by exactly one blank line.
pub fn strip_revision_section(body: &str) -> String {
    let Some(heading_pos) = find_heading_at_line_start(body, REVISION_TABLE_HEADING) else {
        return body.to_string();
    };
    let after_heading = &body[heading_pos + REVISION_TABLE_HEADING.len()..];
    let cut_end = match find_table_end_offset(after_heading) {
        Some(rel) => heading_pos + REVISION_TABLE_HEADING.len() + rel,
        None => {
            // Heading present without a parseable table — strip only
            // the heading line so a malformed section doesn't survive.
            body[heading_pos..]
                .find('\n')
                .map(|i| heading_pos + i + 1)
                .unwrap_or(body.len())
        }
    };
    let mut out = String::with_capacity(body.len());
    out.push_str(&body[..heading_pos]);
    out.push_str(&body[cut_end..]);
    // Collapse 3+ consecutive newlines that the cut may have created
    // at the seam down to 2.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

/// Insert `row` into the body's revision-history table, creating the
/// section if it doesn't exist. New rows go at the bottom of the
/// table (chronological); a brand-new section sits **above** the
/// first trailing `<details>` block so the cascade-stashed full-body
/// revisions stay at the very end of the file.
pub fn append_revision_row(body: &str, row: RevisionRow) -> String {
    let row_md = render_row(&row);

    if let Some(heading_pos) = find_heading_at_line_start(body, REVISION_TABLE_HEADING) {
        // Section exists. Find where the table ends (first blank line
        // or non-pipe line after the separator) and splice the new row
        // in just before that boundary.
        let head_end = heading_pos + REVISION_TABLE_HEADING.len();
        if let Some(table_end_rel) = find_table_end_offset(&body[head_end..]) {
            let abs_end = head_end + table_end_rel;
            let mut out = String::with_capacity(body.len() + row_md.len() + 2);
            out.push_str(&body[..abs_end]);
            // The slice ends exactly after the last table row's
            // newline (or at the boundary if the file ends mid-table).
            // Append the new row then continue with the rest.
            out.push_str(&row_md);
            out.push('\n');
            out.push_str(&body[abs_end..]);
            return out;
        }
        // Heading present but no table after it — rebuild the whole
        // section right after the heading.
    }

    // No existing section. Insert before the earliest trailing
    // managed block — either the first `<details>` block (cascade's
    // stashed prior bodies) or the TOC sentinel (auto-managed
    // Contents section). Both are written by the editor /
    // cascade-runner / TOC effect and treat everything from their
    // start to EOF as their own territory, so a revision section
    // placed AFTER either gets stomped on the next round-trip.
    let insertion_point = find_first_trailing_managed_block(body).unwrap_or(body.len());
    let mut out = String::with_capacity(body.len() + 256);
    out.push_str(&body[..insertion_point]);
    // Ensure exactly one blank line before the heading.
    let prefix = &body[..insertion_point];
    if !prefix.ends_with("\n\n") {
        if prefix.ends_with('\n') {
            out.push('\n');
        } else if !prefix.is_empty() {
            out.push_str("\n\n");
        }
    }
    out.push_str(REVISION_TABLE_HEADING);
    out.push('\n');
    out.push('\n');
    out.push_str(TABLE_COLUMNS_HEADER);
    out.push('\n');
    out.push_str(TABLE_COLUMNS_SEPARATOR);
    out.push('\n');
    out.push_str(&row_md);
    out.push('\n');
    if insertion_point < body.len() {
        // Re-insert a separating blank line if there's content after
        // (typically a `<details>` block).
        if !body[insertion_point..].starts_with("\n") {
            out.push('\n');
        }
    }
    out.push_str(&body[insertion_point..]);
    out
}

// ---------- helpers ----------

fn render_row(r: &RevisionRow) -> String {
    format!(
        "| {} | {} | {} | {} |",
        r.revision,
        escape_cell(&r.date),
        escape_cell(&r.derived_from),
        escape_cell(&r.summary),
    )
}

fn escape_cell(s: &str) -> String {
    // Pipes break the table; newlines do too. Collapse both. The
    // escape `\|` is the markdown-table convention; readers (including
    // the rendered preview) unescape it correctly.
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.trim().to_string()
}

fn unescape_cell(s: &str) -> String {
    // Reverse the escape applied by `escape_cell`: `\|` → `|`. Note
    // that `split_cells` strips literal `\|` sequences entirely
    // because it splits naively on `|` — so callers building rows
    // from `split_cells` output never see `\|` in the captured cell
    // text. This function is the safety net for cells that *do* still
    // carry the backslash (e.g. a hand-edited row pasted as-is).
    s.replace("\\|", "|").trim().to_string()
}

fn find_heading_at_line_start(body: &str, heading: &str) -> Option<usize> {
    // Match `^heading$` (with optional trailing whitespace) at any
    // line boundary. Avoids matching `### Revision history` (heading
    // is `## `) and avoids embedded matches inside code blocks by the
    // crude-but-effective rule "must be a full line".
    let mut pos = 0usize;
    for line in body.split('\n') {
        let trimmed = line.trim_end();
        if trimmed == heading {
            return Some(pos);
        }
        pos += line.len() + 1; // +1 for the consumed '\n'
    }
    None
}

fn is_table_header(line: &str) -> bool {
    // Must start with `|`, end with `|`, and contain the four expected
    // column names in order (case-insensitive). Whitespace within
    // cells is tolerated.
    if !line.starts_with('|') || !line.ends_with('|') {
        return false;
    }
    let cells = split_cells(line);
    if cells.len() < 4 {
        return false;
    }
    cells[0].eq_ignore_ascii_case("Revision")
        && cells[1].eq_ignore_ascii_case("Date")
        && cells[2].eq_ignore_ascii_case("Derived from")
        && cells[3].eq_ignore_ascii_case("Summary")
}

fn is_table_separator(line: &str) -> bool {
    if !line.starts_with('|') || !line.ends_with('|') {
        return false;
    }
    let cells = split_cells(line);
    if cells.is_empty() {
        return false;
    }
    cells.iter().all(|c| {
        let t = c.trim();
        !t.is_empty() && t.chars().all(|ch| ch == '-' || ch == ':')
    })
}

/// Split a `| a | b | c |` table row into trimmed cell strings,
/// respecting `\|` as an escaped pipe (cell text, not a separator).
/// Returns owned strings so the caller can unescape independently.
fn split_cells(line: &str) -> Vec<String> {
    let inner = line.trim_start_matches('|').trim_end_matches('|');
    let mut cells: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            // Escaped pipe — keep both chars in the cell so
            // `unescape_cell` can resolve to a literal `|`.
            current.push(ch);
            current.push(chars.next().unwrap());
        } else if ch == '|' {
            cells.push(current.trim().to_string());
            current = String::new();
        } else {
            current.push(ch);
        }
    }
    cells.push(current.trim().to_string());
    cells
}

fn parse_data_row(line: &str) -> Option<RevisionRow> {
    let cells: Vec<String> = split_cells(line)
        .into_iter()
        .map(|c| unescape_cell(&c))
        .collect();
    if cells.len() < 4 {
        return None;
    }
    let revision: i64 = cells[0].parse().ok()?;
    Some(RevisionRow {
        revision,
        date: cells[1].clone(),
        derived_from: cells[2].clone(),
        summary: cells[3].clone(),
    })
}

/// Returns the byte offset within `after_heading` at which the table
/// ends (one past the last `\n` after a table row, or the end of the
/// input if the table runs to EOF). `None` when the header/separator
/// don't parse.
fn find_table_end_offset(after_heading: &str) -> Option<usize> {
    // Walk lines, accumulating offsets. End = offset of the first
    // post-table line (blank or non-pipe).
    let mut offset = 0usize;
    let mut state = State::PreHeader;
    enum State {
        PreHeader,
        SawHeader,
        InRows,
    }
    for line in after_heading.split('\n') {
        let consumed = line.len() + 1; // +1 for the '\n'
        let t = line.trim();
        match state {
            State::PreHeader => {
                if t.is_empty() {
                    // skip
                } else if is_table_header(t) {
                    state = State::SawHeader;
                } else {
                    return None;
                }
            }
            State::SawHeader => {
                if is_table_separator(t) {
                    state = State::InRows;
                } else {
                    return None;
                }
            }
            State::InRows => {
                if t.is_empty() || !t.starts_with('|') {
                    // Table ended at the start of this line.
                    return Some(offset);
                }
            }
        }
        offset += consumed;
    }
    // Ran to EOF in a valid table position.
    match state {
        State::InRows => Some(offset.saturating_sub(1)),
        _ => None,
    }
}

/// Earliest byte offset of any "trailing managed block" the
/// revision section needs to sit before. Considers both:
///   - the first trailing `<details>` block (cascade-stashed prior
///     bodies)
///   - the `<!-- operon:toc -->` sentinel (auto-managed Contents
///     section; everything from the sentinel to EOF is owned by
///     `src/plugins/toc`).
///
/// Returns `None` when neither is present, which means
/// `append_revision_row` falls back to appending at end-of-body.
fn find_first_trailing_managed_block(body: &str) -> Option<usize> {
    let details = find_first_trailing_details(body);
    let toc = body.find(crate::plugins::toc::TOC_SENTINEL);
    match (details, toc) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find_first_trailing_details(body: &str) -> Option<usize> {
    // Lower-cased scan that stops at the first `<details` token that
    // is preceded (after trimming whitespace and blank lines) by no
    // further non-`<details>` markdown. Heuristic: find the *last*
    // contiguous run of `<details>...</details>` blocks at the end of
    // the body; the insertion point is the start of the first one in
    // that run. If a `<details>` is interleaved with prose, we don't
    // treat it as "trailing".
    //
    // Simpler heuristic that's good enough: find the first `<details`
    // tag after which only `<details>` blocks and whitespace appear.
    let lower = body.to_ascii_lowercase();
    let mut search_from = 0;
    let mut first_trailing: Option<usize> = None;
    while let Some(rel) = lower[search_from..].find("<details") {
        let abs = search_from + rel;
        // Look at what's between the *previous* non-details boundary
        // and this match — if it's only whitespace/blank lines/other
        // `</details>` closes, this is part of the trailing block run.
        // Keep moving forward until we either find one with prose
        // between or we reach EOF.
        first_trailing = Some(abs);
        // Advance past the matching close so we keep walking the chain.
        if let Some(end_rel) = lower[abs..].find("</details>") {
            search_from = abs + end_rel + "</details>".len();
        } else {
            break;
        }
        // Anything between `search_from` and the next `<details` that
        // isn't whitespace voids the "trailing" status.
        let gap = &body[search_from..];
        let next_details_rel = lower[search_from..].find("<details");
        let limit = next_details_rel.unwrap_or(gap.len());
        if !gap[..limit].trim().is_empty() {
            // Prose between two `<details>` blocks — the first one
            // isn't actually trailing. Reset.
            first_trailing = None;
        }
    }
    first_trailing
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_on_missing_heading() {
        assert!(parse_revision_table("just some prose\n").is_none());
        assert!(parse_revision_table("").is_none());
    }

    #[test]
    fn parse_returns_none_on_heading_without_table() {
        let body = "## Revision history\n\nno table here, just prose.\n";
        assert!(parse_revision_table(body).is_none());
    }

    #[test]
    fn parse_does_not_match_nested_or_wrong_level_heading() {
        let body = "### Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-01-01 | manual | hi |\n";
        assert!(parse_revision_table(body).is_none());
    }

    #[test]
    fn parse_well_formed_table() {
        let body = "\
intro prose
## Revision history

| Revision | Date       | Derived from | Summary       |
|----------|------------|--------------|---------------|
| 1        | 2026-05-11 | manual       | Initial draft.|
| 2        | 2026-05-12 | claude       | Added section.|

trailing prose
";
        let t = parse_revision_table(body).expect("table parses");
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].revision, 1);
        assert_eq!(t.rows[0].date, "2026-05-11");
        assert_eq!(t.rows[0].derived_from, "manual");
        assert_eq!(t.rows[0].summary, "Initial draft.");
        assert_eq!(t.rows[1].revision, 2);
        assert_eq!(t.rows[1].derived_from, "claude");
    }

    #[test]
    fn parse_unescapes_pipes_in_cells() {
        let body = "## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-01-01 | manual | needs \\| pipe handling |\n";
        let t = parse_revision_table(body).unwrap();
        assert_eq!(t.rows[0].summary, "needs | pipe handling");
    }

    #[test]
    fn next_revision_number_starts_at_one() {
        assert_eq!(next_revision_number(""), 1);
        assert_eq!(next_revision_number("body without table\n"), 1);
    }

    #[test]
    fn next_revision_number_after_existing_table() {
        let body = "## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-11 | manual | first |\n| 2 | 2026-05-12 | manual | second |\n";
        assert_eq!(next_revision_number(body), 3);
    }

    #[test]
    fn next_revision_number_handles_gaps() {
        let body = "## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-11 | manual | one |\n| 5 | 2026-05-12 | manual | five |\n";
        assert_eq!(next_revision_number(body), 6);
    }

    #[test]
    fn append_creates_section_when_absent() {
        let body = "Some body content here.\n";
        let row = RevisionRow {
            revision: 1,
            date: "2026-05-11".into(),
            derived_from: "manual".into(),
            summary: "Initial draft.".into(),
        };
        let out = append_revision_row(body, row);
        assert!(out.contains("## Revision history"));
        assert!(out.contains("| 1 | 2026-05-11 | manual | Initial draft. |"));
        // Existing content survives.
        assert!(out.starts_with("Some body content here."));
        // The section is the only revision history.
        assert_eq!(out.matches("## Revision history").count(), 1);
        // And it parses back.
        let parsed = parse_revision_table(&out).unwrap();
        assert_eq!(parsed.rows.len(), 1);
    }

    #[test]
    fn append_extends_existing_table() {
        let body = "Body.\n\n## Revision history\n\n| Revision | Date       | Derived from | Summary       |\n|----------|------------|--------------|---------------|\n| 1        | 2026-05-11 | manual       | Initial draft.|\n";
        let row = RevisionRow {
            revision: 2,
            date: "2026-05-12".into(),
            derived_from: "claude".into(),
            summary: "Added section.".into(),
        };
        let out = append_revision_row(body, row);
        let parsed = parse_revision_table(&out).unwrap();
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[1].derived_from, "claude");
        assert_eq!(parsed.rows[1].summary, "Added section.");
    }

    #[test]
    fn append_inserts_above_toc_sentinel() {
        // Reproduces the bug where artifact Revise → Done changes
        // appeared to revert: the section landed after the TOC
        // sentinel, and the next `refresh_if_managed` round-trip
        // (triggered by the LOCAL_NOTE_VERSION bump from the same
        // save) wiped everything from the sentinel to EOF — including
        // our brand-new revision row.
        let body = "Body content.\n\n<!-- operon:toc -->\n## Contents\n\n- child link\n";
        let row = RevisionRow {
            revision: 1,
            date: "2026-05-15".into(),
            derived_from: "manual".into(),
            summary: "Initial draft.".into(),
        };
        let out = append_revision_row(body, row);
        let table_idx = out.find("## Revision history").unwrap();
        let toc_idx = out.find("<!-- operon:toc -->").unwrap();
        assert!(
            table_idx < toc_idx,
            "revision history table must precede the TOC sentinel; got:\n{out}"
        );
        // Round-trip through refresh_if_managed (no notes → empty TOC).
        // The section must survive intact.
        let refreshed = crate::plugins::toc::refresh_if_managed(&out, uuid::Uuid::new_v4(), &[]);
        assert!(
            refreshed.contains("## Revision history"),
            "revision section survives a TOC refresh; got:\n{refreshed}"
        );
        assert!(
            refreshed.contains("| 1 | 2026-05-15 | manual | Initial draft. |"),
            "revision row survives a TOC refresh; got:\n{refreshed}"
        );
    }

    #[test]
    fn append_inserts_above_trailing_details_block() {
        let body = "Body.\n\n<details><summary>Revision 1 (2026-05-10)</summary>\n\nold body\n\n</details>\n";
        let row = RevisionRow {
            revision: 1,
            date: "2026-05-11".into(),
            derived_from: "manual".into(),
            summary: "Initial draft.".into(),
        };
        let out = append_revision_row(body, row);
        let table_idx = out.find("## Revision history").unwrap();
        let details_idx = out.find("<details").unwrap();
        assert!(
            table_idx < details_idx,
            "table must precede the existing details block; got:\n{out}"
        );
        // The details block still survives intact.
        assert!(out.contains("<details><summary>Revision 1 (2026-05-10)</summary>"));
    }

    #[test]
    fn escape_protects_pipes_in_summary_round_trip() {
        let body = "Body.\n";
        let row = RevisionRow {
            revision: 1,
            date: "2026-05-11".into(),
            derived_from: "manual".into(),
            summary: "a|b|c summary".into(),
        };
        let out = append_revision_row(body, row);
        assert!(out.contains("a\\|b\\|c summary"));
        let parsed = parse_revision_table(&out).unwrap();
        assert_eq!(parsed.rows[0].summary, "a|b|c summary");
    }

    #[test]
    fn body_has_table_matches_parse() {
        assert!(!body_has_table("nothing here"));
        let with = "## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-11 | manual | initial |\n";
        assert!(body_has_table(with));
    }

    #[test]
    fn format_revision_date_shape() {
        // 1970-01-01 00:00:00 UTC = 0 — anchor of the days-from-civil
        // formula, sanity check.
        assert_eq!(format_revision_date(0), "1970-01-01");
        // 2026-05-11 00:00:00 UTC = 20584 days * 86_400_000 ms/day
        //   = 1_778_457_600_000. Cross-checked against the algorithm
        //   trace: 56y * 365 + 14 leap days + 130 days into 2026.
        assert_eq!(format_revision_date(1_778_457_600_000), "2026-05-11");
    }

    #[test]
    fn format_revision_date_rejects_bogus_input() {
        assert!(format_revision_date(-1).starts_with('@'));
    }

    #[test]
    fn compute_summary_initial_save() {
        assert_eq!(
            compute_summary(None, Some("a\nb\nc")),
            "Initial save (3 lines)"
        );
    }

    #[test]
    fn compute_summary_added() {
        assert_eq!(
            compute_summary(Some("a\nb"), Some("a\nb\nc")),
            "Added 1 line(s) (2 → 3)"
        );
    }

    #[test]
    fn compute_summary_removed() {
        assert_eq!(
            compute_summary(Some("a\nb\nc"), Some("a")),
            "Removed 2 line(s) (3 → 1)"
        );
    }

    #[test]
    fn compute_summary_edited_same_line_count() {
        assert_eq!(compute_summary(Some("alpha"), Some("beta")), "Edited body (1 lines)");
    }

    #[test]
    fn strip_noop_when_no_section() {
        let body = "# Heading\n\nbody text\n";
        assert_eq!(strip_revision_section(body), body);
    }

    #[test]
    fn strip_removes_section_in_middle() {
        let body = "# Heading\n\nintro body\n\n## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-15 | manual | Initial. |\n\nmore body\n";
        let out = strip_revision_section(body);
        assert!(!out.contains("Revision history"));
        assert!(!out.contains("| 1 |"));
        assert!(out.contains("intro body"));
        assert!(out.contains("more body"));
        // Exactly one blank line between intro and more.
        assert!(out.contains("intro body\n\nmore body"));
    }

    #[test]
    fn strip_removes_section_at_eof() {
        let body = "# Heading\n\nbody\n\n## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-15 | manual | Initial. |\n";
        let out = strip_revision_section(body);
        assert!(!out.contains("Revision history"));
        assert!(out.contains("body"));
        // No trailing blank lines accumulated at the cut.
        assert!(!out.ends_with("\n\n\n"));
    }

    #[test]
    fn strip_preserves_trailing_details_blocks() {
        let body = "Body.\n\n## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-15 | manual | Initial. |\n\n<details><summary>Revision 1 (2026-05-10)</summary>\nold body\n</details>\n";
        let out = strip_revision_section(body);
        assert!(!out.contains("## Revision history"));
        // The cascade-stashed <details> block must survive.
        assert!(out.contains("<details><summary>Revision 1 (2026-05-10)</summary>"));
        assert!(out.contains("old body"));
    }

    #[test]
    fn strip_idempotent() {
        let body = "Body.\n\n## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-15 | manual | Initial. |\n";
        let once = strip_revision_section(body);
        let twice = strip_revision_section(&once);
        assert_eq!(once, twice);
    }
}
