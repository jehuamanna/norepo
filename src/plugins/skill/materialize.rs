//! Write a skill note's body to `<repo>/.claude/skills/<slug>.md` so
//! Claude Code's native skill loader can resolve it on the next turn.
//! Operon owns the round-trip — the skill note in SQLite is the source
//! of truth; the materialized `.md` is a derived cache rewritten on
//! every Play.
//!
//! **Frontmatter compatibility (M1):** Claude Code's skill loader
//! requires the top-level YAML keys `name:` and `description:` to
//! discover and load a skill. Operon's authoring model uses
//! `skill_name:` plus a richer set of SDLC fields
//! (`input_kind` / `output_kind` / `aggregate` / `inherit` / etc.).
//! `write_skill_to_repo` runs the body through `to_claude_compat`,
//! which injects `name:` (from the file slug) and `description:`
//! (synthesized from the contract when not user-provided), leaving
//! every other field — Claude-known or Operon-only — in place.
//! Claude silently ignores unknown keys, so the SDLC metadata
//! survives intact.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::frontmatter;

#[derive(Debug)]
pub enum MaterializeError {
    Io(io::Error),
    EmptyBody,
}

impl std::fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::EmptyBody => write!(f, "skill body is empty"),
        }
    }
}

impl std::error::Error for MaterializeError {}

impl From<io::Error> for MaterializeError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Returns the absolute path of the materialized file on success.
pub fn write_skill_to_repo(
    repo_path: &Path,
    slug: &str,
    body: &str,
) -> Result<PathBuf, MaterializeError> {
    if body.trim().is_empty() {
        return Err(MaterializeError::EmptyBody);
    }
    let dir = repo_path.join(".claude").join("skills");
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{slug}.md"));
    let compat = to_claude_compat(body, slug);
    fs::write(&target, compat)?;
    Ok(target)
}

/// Rewrite a skill body so its leading YAML frontmatter satisfies
/// Claude Code's skill-loader contract: top-level `name:` and
/// `description:` keys must be present. Missing keys are injected at
/// the head of the block; existing values are preserved untouched
/// (idempotent on already-compliant input). Operon-only fields
/// (`skill_name`, `input_kind`, `aggregate`, …) survive unchanged —
/// Claude ignores unknown keys, so the SDLC contract still rides
/// through.
///
/// The synthesized `description:` is derived from the contract
/// (`persona` + `input_kind` → `output_kind` when available) so
/// Claude's discovery prompt has something specific to match against,
/// rather than the bare slug. Authors can override by adding their
/// own `description:` line to the skill note's frontmatter.
pub fn to_claude_compat(body: &str, slug: &str) -> String {
    let (fm_borrow, rest) = frontmatter::split(body);
    let fm_lines: Vec<String> = fm_borrow
        .map(|v| v.into_iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let has_name = fm_lines.iter().any(|l| line_has_key(l, "name"));
    let has_description = fm_lines.iter().any(|l| line_has_key(l, "description"));

    // Strip the in-body `## Revision history` table. The table is
    // editor-only UI metadata (what the user / Claude have changed
    // across revisions) and is meaningless — actively confusing —
    // inside the skill prompt that Claude Code's skill loader reads.
    let rest = crate::plugins::artifact::revision_table::strip_revision_section(rest);

    let mut out = String::from("---\n");
    if !has_name {
        out.push_str(&format!("name: {slug}\n"));
    }
    if !has_description {
        let desc = synthesize_description(&fm_lines, slug);
        out.push_str(&format!("description: {}\n", yaml_inline_quote(&desc)));
    }
    for line in &fm_lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("---\n\n");
    out.push_str(&rest);
    out
}

/// `true` when `line` is a top-level `key:` (or `key: ...`) assignment
/// — ignores indented lines so we don't false-match a nested key with
/// the same name.
fn line_has_key(line: &str, key: &str) -> bool {
    if line.starts_with(|c: char| c == ' ' || c == '\t') {
        return false;
    }
    line.split_once(':')
        .map(|(k, _)| k.trim() == key)
        .unwrap_or(false)
}

/// Build a one-line description from the skill's contract fields.
/// Falls back to a humanised slug ("ba decompose epic" → "Ba
/// decompose epic") when no contract metadata is available.
fn synthesize_description(fm: &[String], slug: &str) -> String {
    let refs: Vec<&str> = fm.iter().map(|s| s.as_str()).collect();
    let persona = frontmatter::field(&refs, "persona");
    let input = frontmatter::field(&refs, "input_kind");
    let output = frontmatter::field(&refs, "output_kind");
    let many = matches!(
        frontmatter::field(&refs, "output_count"),
        Some("many" | "Many" | "MANY")
    );
    let plural = if many { "multiple " } else { "" };
    match (persona, input, output) {
        (Some(p), Some(i), Some(o)) => {
            format!("{p} skill: produces {plural}{o} artifact(s) from a {i}")
        }
        (None, Some(i), Some(o)) => format!("Produces {plural}{o} artifact(s) from a {i}"),
        (Some(p), None, Some(o)) => format!("{p} skill: produces {plural}{o} artifact(s)"),
        (None, None, Some(o)) => format!("Produces {plural}{o} artifact(s)"),
        (Some(p), Some(i), None) => format!("{p} skill operating on {i} artifacts"),
        (Some(p), None, None) => format!("{p} skill"),
        _ => humanize_slug(slug),
    }
}

fn humanize_slug(slug: &str) -> String {
    let spaced = slug.replace('-', " ").replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Operon skill".into(),
    }
}

/// Quote an inline YAML scalar when it contains characters that
/// would otherwise confuse a strict parser. Conservative: skip the
/// quotes only when the value is a plain run of safe characters.
fn yaml_inline_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && !s.starts_with(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '?' | ':' | '*' | '&' | '!' | '#' | '|' | '>' | '\'' | '"' | '%' | '@' | '`'
                )
        })
        && !s.contains(['\n', '\r', '#', ':']);
    if safe {
        s.to_string()
    } else {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }
}

/// Remove a previously-materialized skill file. No-op if it didn't exist.
pub fn remove_skill_from_repo(repo_path: &Path, slug: &str) -> Result<(), MaterializeError> {
    let target = repo_path.join(".claude").join("skills").join(format!("{slug}.md"));
    if target.exists() {
        fs::remove_file(target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_creates_dir_and_file() {
        let tmp = tempdir().unwrap();
        let path = write_skill_to_repo(tmp.path(), "ba-intake", "you are a BA").unwrap();
        assert!(path.is_file());
        let body = fs::read_to_string(&path).unwrap();
        // Materialized file is wrapped with Claude-compliant frontmatter
        // so the skill loader can discover it. Body content is preserved
        // after the closing fence.
        assert!(body.starts_with("---\nname: ba-intake\n"));
        assert!(body.contains("description:"));
        assert!(body.contains("you are a BA"));
        let dir = tmp.path().join(".claude").join("skills");
        assert!(dir.is_dir());
    }

    #[test]
    fn write_overwrites_existing() {
        let tmp = tempdir().unwrap();
        write_skill_to_repo(tmp.path(), "x", "v1").unwrap();
        let path = write_skill_to_repo(tmp.path(), "x", "v2").unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("v2"));
        assert!(!contents.contains("v1"));
    }

    #[test]
    fn write_rejects_empty_body() {
        let tmp = tempdir().unwrap();
        let err = write_skill_to_repo(tmp.path(), "x", "   \n").unwrap_err();
        assert!(matches!(err, MaterializeError::EmptyBody));
    }

    #[test]
    fn remove_is_noop_for_missing() {
        let tmp = tempdir().unwrap();
        // No file was ever written.
        remove_skill_from_repo(tmp.path(), "nonexistent").unwrap();
    }

    #[test]
    fn remove_deletes_existing() {
        let tmp = tempdir().unwrap();
        let path = write_skill_to_repo(tmp.path(), "doomed", "body").unwrap();
        remove_skill_from_repo(tmp.path(), "doomed").unwrap();
        assert!(!path.exists());
    }

    // ===== to_claude_compat coverage =====

    #[test]
    fn compat_wraps_bare_body_with_full_frontmatter() {
        let out = to_claude_compat("just the prose body\n", "my-skill");
        assert!(out.starts_with("---\nname: my-skill\n"));
        assert!(out.contains("description:"));
        assert!(out.ends_with("just the prose body\n"));
    }

    #[test]
    fn compat_preserves_existing_name_and_description() {
        let body = "---\n\
            name: explicit-name\n\
            description: a hand-written description\n\
            skill_name: ba-intake\n\
            ---\n\nbody";
        let out = to_claude_compat(body, "fallback-slug");
        // No duplicate top-level name/description injection. Count
        // line-starts (anchored) so `skill_name:` doesn't false-match
        // the substring `name:`.
        let name_lines = out.lines().filter(|l| l.starts_with("name:")).count();
        let desc_lines = out.lines().filter(|l| l.starts_with("description:")).count();
        assert_eq!(name_lines, 1);
        assert_eq!(desc_lines, 1);
        assert!(out.contains("name: explicit-name"));
        assert!(out.contains("description: a hand-written description"));
        // Operon-only fields ride through unchanged.
        assert!(out.contains("skill_name: ba-intake"));
    }

    #[test]
    fn compat_injects_only_missing_keys() {
        // Description present, name missing.
        let body = "---\n\
            description: keep this\n\
            skill_name: ba-decompose-epic\n\
            input_kind: epic\n\
            output_kind: story\n\
            ---\n\nbody";
        let out = to_claude_compat(body, "ba-decompose-epic");
        assert!(out.contains("name: ba-decompose-epic"));
        // Only one description, the user's.
        let desc_lines = out.lines().filter(|l| l.starts_with("description:")).count();
        assert_eq!(desc_lines, 1);
        assert!(out.contains("description: keep this"));
    }

    #[test]
    fn compat_synthesizes_description_from_contract() {
        let body = "---\n\
            skill_name: ba-decompose-epic\n\
            input_kind: epic\n\
            output_kind: story\n\
            output_count: many\n\
            persona: BA\n\
            ---\n\nbody";
        let out = to_claude_compat(body, "ba-decompose-epic");
        let desc_line = out
            .lines()
            .find(|l| l.starts_with("description:"))
            .expect("description injected");
        // Persona, kinds, and many/fan-out plurality all surface.
        assert!(desc_line.contains("BA"), "{desc_line}");
        assert!(desc_line.contains("epic"), "{desc_line}");
        assert!(desc_line.contains("story"), "{desc_line}");
        assert!(desc_line.contains("multiple"), "{desc_line}");
    }

    #[test]
    fn compat_synthesizes_description_humanizes_slug_without_contract() {
        let body = "---\nskill_name: random-skill\n---\n\nbody";
        let out = to_claude_compat(body, "random-skill");
        let desc_line = out
            .lines()
            .find(|l| l.starts_with("description:"))
            .unwrap();
        assert!(desc_line.contains("Random skill"), "{desc_line}");
    }

    #[test]
    fn compat_is_idempotent_on_already_compliant_body() {
        let body = "---\nname: ba-intake\ndescription: take a BA intake\n---\n\nyou are a BA\n";
        let once = to_claude_compat(body, "ba-intake");
        let twice = to_claude_compat(&once, "ba-intake");
        assert_eq!(once, twice);
    }

    #[test]
    fn compat_strips_revision_history_from_body() {
        // The revision-history table is in-app metadata and must not
        // leak into the materialized prompt Claude Code reads.
        let body = "---\nskill_name: ba-intake\n---\n\nYou are a BA.\n\n## Revision history\n\n| Revision | Date | Derived from | Summary |\n|-|-|-|-|\n| 1 | 2026-05-15 | manual | First. |\n";
        let out = to_claude_compat(body, "ba-intake");
        assert!(out.contains("You are a BA."));
        assert!(!out.contains("Revision history"));
        assert!(!out.contains("manual"));
        assert!(!out.contains("| 1 |"));
    }

    #[test]
    fn compat_quotes_description_with_colons_or_hashes() {
        // Synthesized description contains "skill:" — must be quoted so
        // Claude's YAML parser doesn't read it as a nested mapping.
        let body = "---\npersona: BA\ninput_kind: epic\noutput_kind: story\n---\n\nbody";
        let out = to_claude_compat(body, "x");
        let desc_line = out.lines().find(|l| l.starts_with("description:")).unwrap();
        // After `description:` the value must be wrapped in double quotes.
        let value = desc_line.trim_start_matches("description:").trim();
        assert!(value.starts_with('"') && value.ends_with('"'), "{desc_line}");
    }
}
