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

/// Split a note's content into `(frontmatter_lines, body)`.
/// Returns `(None, full_content)` when the content doesn't begin with
/// a `---` fence — every other line stays in the body verbatim.
///
/// Lenient to **stacked** frontmatter: if the body after the first
/// `---...---` block immediately starts with another `---` fence, the
/// parser folds the second block's lines into the first and so on
/// until a non-frontmatter line is reached. This handles the
/// "user-pasted on top of an existing block" foot-gun where a body
/// ends up with two consecutive frontmatter sections; the artifact
/// view's `rewrite()` self-heals it on the next save by emitting a
/// single canonical block.
pub fn split(content: &str) -> (Option<Vec<&str>>, &str) {
    let mut all_lines: Vec<&str> = Vec::new();
    let mut remaining = content;
    let mut found_any = false;
    loop {
        match split_one(remaining) {
            (Some(block_lines), next_body) => {
                all_lines.extend(block_lines);
                remaining = next_body;
                found_any = true;
            }
            (None, _) => break,
        }
    }
    if found_any {
        (Some(all_lines), remaining)
    } else {
        (None, content)
    }
}

/// Single-block splitter — extracts exactly one leading `---...---`
/// frontmatter block from `content` and returns its line slice + the
/// body that follows. Returns `(None, content)` when no leading block
/// is present. The public `split` calls this in a loop to support
/// stacked blocks.
fn split_one(content: &str) -> (Option<Vec<&str>>, &str) {
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

/// SDLC pipeline contract: declared in a skill's frontmatter so the
/// engine knows which artifact kinds the skill consumes / produces,
/// whether a single run produces one or many output artifacts, and
/// whether the user must approve outputs before downstream skills can
/// run. All fields default sensibly when absent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillContract {
    /// Artifact kind the skill expects as input. Strings here mirror
    /// `ArtifactKind` ("epic", "feature", "story", "task",
    /// "requirements", …); kept as a free string so a skill can
    /// declare a custom kind without forcing a code change.
    pub input_kind: Option<String>,
    /// Artifact kind the skill produces.
    pub output_kind: Option<String>,
    /// "one" → exactly one artifact note per run (Plan / TestCases /
    /// Summary). "many" → fan out: every Write tool call inside the
    /// project's `Artifacts/` directory becomes a sibling artifact
    /// note. Default is "one" so existing skills don't change shape.
    pub output_count: SkillOutputCount,
    /// Whether the user must explicitly approve produced artifacts
    /// before downstream skills become eligible. Default is gated.
    pub gate: SkillGate,
    /// Optional persona label (BA / Architect / QA / Engineer) — the
    /// engine treats this as opaque metadata; the artifact view uses
    /// it for a small badge.
    pub persona: Option<String>,
    /// Aggregator skill: when set, the runner walks the source seed's
    /// descendant tree and inlines every artifact with this kind into
    /// the prompt instead of just the source body. Used by
    /// prioritization / cross-task analysis skills that need to see
    /// every Task (or every Plan) under the seed at once.
    pub aggregate: Option<String>,
    /// Cascade checkpoint: when `true`, the cascade orchestrator does
    /// NOT auto-approve this skill's produced artifacts, so the chain
    /// pauses on them and waits for explicit user approval. Lets a
    /// pipeline insert human-review gates without separating the run.
    pub cascade_stop: bool,
    /// Workflow emission: when `true`, the runner parses the
    /// produced artifact for a `## Priority order` (and optional
    /// dependency hints) and creates a sibling `NoteKind::Workflow`
    /// note holding a React-Flow DAG snapshot of the prioritized
    /// tasks. Used by the prioritization skills.
    pub emit_workflow: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillOutputCount {
    #[default]
    One,
    Many,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillGate {
    /// Outputs land in `pending` status and must be Approved before
    /// downstream skills run on them.
    #[default]
    Approval,
    /// Outputs land in `approved` immediately. Use for low-risk
    /// summarizers where review is unnecessary.
    Auto,
}

/// Read the contract fields from a skill note's frontmatter. Always
/// succeeds; missing keys land as `None`/defaults.
pub fn contract(lines: &[&str]) -> SkillContract {
    let input_kind = field(lines, "input_kind").map(str::to_string);
    let output_kind = field(lines, "output_kind").map(str::to_string);
    let output_count = match field(lines, "output_count") {
        Some("many") | Some("Many") | Some("MANY") => SkillOutputCount::Many,
        _ => SkillOutputCount::One,
    };
    let gate = match field(lines, "gate") {
        Some("auto") | Some("Auto") | Some("AUTO") => SkillGate::Auto,
        _ => SkillGate::Approval,
    };
    let persona = field(lines, "persona").map(str::to_string);
    let aggregate = field(lines, "aggregate").map(str::to_string);
    let cascade_stop = matches!(
        field(lines, "cascade_stop"),
        Some("true") | Some("True") | Some("TRUE") | Some("yes") | Some("Yes")
    );
    let emit_workflow = matches!(
        field(lines, "emit_workflow"),
        Some("true") | Some("True") | Some("TRUE") | Some("yes") | Some("Yes")
    );
    SkillContract {
        input_kind,
        output_kind,
        output_count,
        gate,
        persona,
        aggregate,
        cascade_stop,
        emit_workflow,
    }
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
    fn split_merges_two_consecutive_frontmatter_blocks() {
        // The exact "user pasted on top of an existing block" foot-gun.
        // First block has only `status`; second adds `artifact_kind`.
        // After merge, both keys should be discoverable via `field`.
        let s = "---\nstatus: approved\n---\n---\nartifact_kind: requirements\nstatus: approved\n---\n\n# Title\nbody here";
        let (fm, body) = split(s);
        let fm = fm.expect("frontmatter present");
        assert_eq!(field(&fm, "artifact_kind"), Some("requirements"));
        assert_eq!(field(&fm, "status"), Some("approved"));
        assert!(body.starts_with("# Title"));
    }

    #[test]
    fn split_merges_three_consecutive_frontmatter_blocks() {
        // Pathological but the loop should just keep folding.
        let s =
            "---\na: 1\n---\n---\nb: 2\n---\n---\nc: 3\n---\nbody";
        let (fm, body) = split(s);
        let fm = fm.expect("frontmatter present");
        assert_eq!(field(&fm, "a"), Some("1"));
        assert_eq!(field(&fm, "b"), Some("2"));
        assert_eq!(field(&fm, "c"), Some("3"));
        assert_eq!(body, "body");
    }

    #[test]
    fn split_does_not_consume_horizontal_rule_in_body() {
        // A `---` after some markdown content is a horizontal rule,
        // not a stacked frontmatter block. The single-block parser
        // requires the next chars to be `---` immediately at the start
        // of the body (no intervening content), so the loop bails on
        // the first non-frontmatter line — leaving the HR in body.
        let s = "---\nkey: value\n---\n\n# Heading\n\n---\n\nMore body";
        let (fm, body) = split(s);
        let fm = fm.expect("frontmatter present");
        assert_eq!(field(&fm, "key"), Some("value"));
        assert!(body.contains("# Heading"));
        assert!(body.contains("---")); // HR preserved
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
    fn contract_defaults_when_no_fields() {
        let lines: Vec<&str> = vec!["skill_name: x"];
        let c = contract(&lines);
        assert_eq!(c.input_kind, None);
        assert_eq!(c.output_kind, None);
        assert_eq!(c.output_count, SkillOutputCount::One);
        assert_eq!(c.gate, SkillGate::Approval);
    }

    #[test]
    fn contract_reads_all_fields() {
        let lines = vec![
            "skill_name: ba-decompose-epic",
            "input_kind: epic",
            "output_kind: feature",
            "output_count: many",
            "gate: auto",
            "persona: BA",
        ];
        let c = contract(&lines);
        assert_eq!(c.input_kind.as_deref(), Some("epic"));
        assert_eq!(c.output_kind.as_deref(), Some("feature"));
        assert_eq!(c.output_count, SkillOutputCount::Many);
        assert_eq!(c.gate, SkillGate::Auto);
        assert_eq!(c.persona.as_deref(), Some("BA"));
        // Optional new fields default off when absent.
        assert_eq!(c.aggregate, None);
        assert!(!c.cascade_stop);
        assert!(!c.emit_workflow);
    }

    #[test]
    fn contract_reads_checkpoint_fields() {
        let lines = vec![
            "skill_name: pm-prioritize-tasks-coarse",
            "input_kind: requirements",
            "output_kind: prioritized_backlog",
            "aggregate: task",
            "cascade_stop: true",
            "emit_workflow: true",
            "persona: PM",
        ];
        let c = contract(&lines);
        assert_eq!(c.aggregate.as_deref(), Some("task"));
        assert!(c.cascade_stop);
        assert!(c.emit_workflow);
        assert_eq!(c.persona.as_deref(), Some("PM"));
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
