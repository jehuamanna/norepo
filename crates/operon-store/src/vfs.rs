//! Virtual File System helpers shared between the explorer breadcrumb,
//! Plans-Phase-5 wikilink resolver, and any future tooling that needs to
//! map note paths or wikilink text to canonical `NoteId`s.

use uuid::Uuid;

use crate::error::StoreError;
use crate::repos::{LocalNoteRepository, LocalProjectRepository};

/// Parsed wikilink target. Four forms are supported, mirroring the seed:
///
/// - `[[Note]]`                            → `Relative { title }` (resolves
///   against the source note's project).
/// - `[[Project/Note]]`                    → `Absolute { project, title }`.
/// - `[[Project/Parent/.../Note]]`         → `Nested { project, parent_path,
///   title }` — Plans-Phase-9 (rev 2). `parent_path` is the list of
///   intermediate parent-note titles from the project root down (root-most
///   first). Empty `parent_path` is equivalent to `Absolute`.
/// - `[[Note^abc12345]]`                   → `Disambiguated { title, short_id }`
///   — the short id is at least 8 chars of the canonical UUID hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkForm {
    Relative {
        title: String,
    },
    Absolute {
        project: String,
        title: String,
    },
    Nested {
        project: String,
        parent_path: Vec<String>,
        title: String,
        /// Plans-Phase-9-wikilink-picker (rev 2): when the picker emits
        /// `Project/Path/.../Title^abc1234`, this carries the short id
        /// so the resolver can disambiguate same-title duplicates that
        /// happen to share an identical parent chain. `None` for paths
        /// without a `^short` suffix.
        short_id: Option<String>,
    },
    Disambiguated {
        title: String,
        short_id: String,
    },
}

/// Parse the inner text of a wikilink (the part between `[[` and `]]`)
/// into one of the four forms. Returns `None` for empty/whitespace-only
/// text.
pub fn parse_link(text: &str) -> Option<LinkForm> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Strip a `^abc1234` suffix off the WHOLE input, not just the bare
    // title — that lets nested forms like `Project/Folder/Title^abc1234`
    // round-trip cleanly. Anything left of the `^` becomes the path.
    let (path_part, short_id_opt) = match trimmed.rsplit_once('^') {
        Some((before, after)) => {
            let after_lower = after.trim().to_lowercase();
            let before_trimmed = before.trim();
            if !before_trimmed.is_empty()
                && !after_lower.is_empty()
                && after_lower.chars().all(|c| c.is_ascii_hexdigit())
            {
                (before_trimmed, Some(after_lower))
            } else {
                (trimmed, None)
            }
        }
        None => (trimmed, None),
    };
    let segments: Vec<&str> = path_part.split('/').map(str::trim).collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return None;
    }
    match (segments.len(), short_id_opt) {
        (1, None) => Some(LinkForm::Relative {
            title: segments[0].to_string(),
        }),
        (1, Some(short_id)) => Some(LinkForm::Disambiguated {
            title: segments[0].to_string(),
            short_id,
        }),
        (2, None) => Some(LinkForm::Absolute {
            project: segments[0].to_string(),
            title: segments[1].to_string(),
        }),
        (_, short_id) => {
            // 2 segments + short_id, or 3+ segments (with or without
            // short_id), become Nested. The 2+short_id case has an empty
            // parent_path; the resolver scopes by project + title +
            // short_id, which is enough to disambiguate flat duplicates.
            let project = segments[0].to_string();
            let title = segments[segments.len() - 1].to_string();
            let parent_path: Vec<String> = if segments.len() >= 3 {
                segments[1..segments.len() - 1]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect()
            } else {
                Vec::new()
            };
            Some(LinkForm::Nested {
                project,
                parent_path,
                title,
                short_id,
            })
        }
    }
}

/// First eight lower-case hex chars of a UUID — used by the picker to mint
/// disambiguated forms when two notes share a title.
pub fn short_id(id: Uuid) -> String {
    let mut buf = id.simple().to_string();
    buf.truncate(8);
    buf
}

#[derive(Debug)]
pub enum ResolveErr {
    NotFound,
    Ambiguous(Vec<Uuid>),
    Store(StoreError),
}

impl std::fmt::Display for ResolveErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no note matched the wikilink target"),
            Self::Ambiguous(ids) => write!(f, "wikilink target is ambiguous ({} matches)", ids.len()),
            Self::Store(e) => write!(f, "store error during wikilink resolution: {e}"),
        }
    }
}

impl std::error::Error for ResolveErr {}

impl From<StoreError> for ResolveErr {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Resolve a parsed wikilink form to a single `NoteId`. The `source_project_id`
/// is consulted when the form is `Relative` so resolution stays scoped to the
/// source note's project (matching Obsidian's relative-resolution semantics).
pub fn resolve_link(
    projects: &dyn LocalProjectRepository,
    notes: &dyn LocalNoteRepository,
    source_project_id: Uuid,
    form: &LinkForm,
) -> Result<Uuid, ResolveErr> {
    match form {
        LinkForm::Relative { title } => {
            let project_notes = notes.list_for_project(source_project_id)?;
            let matches: Vec<Uuid> = project_notes
                .into_iter()
                .filter(|n| n.title.eq_ignore_ascii_case(title))
                .map(|n| n.id)
                .collect();
            single_match(matches)
        }
        LinkForm::Absolute { project, title } => {
            let projects_list = projects.list()?;
            let project_id = projects_list
                .into_iter()
                .find(|p| p.name.eq_ignore_ascii_case(project))
                .map(|p| p.id);
            let Some(pid) = project_id else {
                return Err(ResolveErr::NotFound);
            };
            let project_notes = notes.list_for_project(pid)?;
            let matches: Vec<Uuid> = project_notes
                .into_iter()
                .filter(|n| n.title.eq_ignore_ascii_case(title))
                .map(|n| n.id)
                .collect();
            single_match(matches)
        }
        LinkForm::Nested {
            project,
            parent_path,
            title,
            short_id,
        } => {
            let projects_list = projects.list()?;
            let project_id = projects_list
                .into_iter()
                .find(|p| p.name.eq_ignore_ascii_case(project))
                .map(|p| p.id);
            let Some(pid) = project_id else {
                return Err(ResolveErr::NotFound);
            };
            let project_notes = notes.list_for_project(pid)?;
            // Walk the parent chain: starting from project-root notes
            // (parent_id = None), each step picks the unique child of the
            // current frontier whose title matches the next path segment.
            // Ambiguity at any level surfaces as `Ambiguous`.
            let mut frontier_ids: Vec<Uuid> = project_notes
                .iter()
                .filter(|n| n.parent_id.is_none())
                .map(|n| n.id)
                .collect();
            for segment in parent_path {
                let next: Vec<Uuid> = project_notes
                    .iter()
                    .filter(|n| {
                        n.parent_id
                            .map(|pid| frontier_ids.contains(&pid))
                            .unwrap_or(false)
                            && n.title.eq_ignore_ascii_case(segment)
                    })
                    .map(|n| n.id)
                    .collect();
                if next.is_empty() {
                    return Err(ResolveErr::NotFound);
                }
                frontier_ids = next;
            }
            // Final segment: leaf with matching title under any frontier id.
            // Empty `parent_path` means the picker referred to a top-level
            // note (`Project/Title` or `Project/Title^short`), so require
            // `parent_id is None`. Non-empty `parent_path` requires the
            // leaf's parent to be in the descended frontier. When the
            // picker emitted `^short`, additionally narrow by the UUID
            // prefix so duplicate-title siblings still resolve uniquely.
            let matches: Vec<Uuid> = project_notes
                .into_iter()
                .filter(|n| {
                    let parent_ok = if parent_path.is_empty() {
                        n.parent_id.is_none()
                    } else {
                        n.parent_id
                            .map(|pid| frontier_ids.contains(&pid))
                            .unwrap_or(false)
                    };
                    let title_ok = n.title.eq_ignore_ascii_case(title);
                    let short_ok = match short_id {
                        Some(s) => n.id.simple().to_string().starts_with(s),
                        None => true,
                    };
                    parent_ok && title_ok && short_ok
                })
                .map(|n| n.id)
                .collect();
            single_match(matches)
        }
        LinkForm::Disambiguated { title, short_id } => {
            // Walk every project — disambiguated forms aren't scoped.
            let projects_list = projects.list()?;
            let mut matches: Vec<Uuid> = Vec::new();
            for p in projects_list {
                for n in notes.list_for_project(p.id)? {
                    if n.title.eq_ignore_ascii_case(title)
                        && n.id.simple().to_string().starts_with(short_id)
                    {
                        matches.push(n.id);
                    }
                }
            }
            single_match(matches)
        }
    }
}

fn single_match(matches: Vec<Uuid>) -> Result<Uuid, ResolveErr> {
    match matches.len() {
        0 => Err(ResolveErr::NotFound),
        1 => Ok(matches[0]),
        _ => Err(ResolveErr::Ambiguous(matches)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_link_empty_returns_none() {
        assert_eq!(parse_link(""), None);
        assert_eq!(parse_link("   "), None);
    }

    #[test]
    fn parse_link_relative() {
        assert_eq!(
            parse_link("Note Title"),
            Some(LinkForm::Relative {
                title: "Note Title".into()
            })
        );
    }

    #[test]
    fn parse_link_absolute() {
        assert_eq!(
            parse_link("Project/Note"),
            Some(LinkForm::Absolute {
                project: "Project".into(),
                title: "Note".into()
            })
        );
    }

    #[test]
    fn parse_link_disambiguated() {
        assert_eq!(
            parse_link("Note^abc12345"),
            Some(LinkForm::Disambiguated {
                title: "Note".into(),
                short_id: "abc12345".into()
            })
        );
    }

    #[test]
    fn parse_link_caret_with_non_hex_falls_back_to_relative() {
        assert_eq!(
            parse_link("Note^xyz"),
            Some(LinkForm::Relative {
                title: "Note^xyz".into()
            })
        );
    }

    #[test]
    fn short_id_is_eight_lower_hex_chars() {
        let id = Uuid::parse_str("ABC12345-DEAD-BEEF-1234-567890123456").unwrap();
        let s = short_id(id);
        assert_eq!(s.len(), 8);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(s, "abc12345");
    }

    // Plans-Phase-9-wikilink-picker (rev 2): nested-path form so the
    // picker can emit `Project / Folder / Sub / Title` references that
    // round-trip through the resolver.
    #[test]
    fn parse_link_nested_three_segments() {
        assert_eq!(
            parse_link("Jehu/Folder/Untitledkkk"),
            Some(LinkForm::Nested {
                project: "Jehu".into(),
                parent_path: vec!["Folder".into()],
                title: "Untitledkkk".into(),
                short_id: None,
            })
        );
    }

    #[test]
    fn parse_link_nested_deep() {
        assert_eq!(
            parse_link("A/B/C/D/E"),
            Some(LinkForm::Nested {
                project: "A".into(),
                parent_path: vec!["B".into(), "C".into(), "D".into()],
                title: "E".into(),
                short_id: None,
            })
        );
    }

    #[test]
    fn parse_link_two_segments_stays_absolute() {
        assert_eq!(
            parse_link("Project/Title"),
            Some(LinkForm::Absolute {
                project: "Project".into(),
                title: "Title".into(),
            })
        );
    }

    #[test]
    fn parse_link_nested_trims_segment_whitespace() {
        assert_eq!(
            parse_link("  Jehu  / Folder /  Untitledkkk  "),
            Some(LinkForm::Nested {
                project: "Jehu".into(),
                parent_path: vec!["Folder".into()],
                title: "Untitledkkk".into(),
                short_id: None,
            })
        );
    }

    // Plans-Phase-9-wikilink-picker (rev 2): short_id suffix on a path
    // tail. The picker emits `Project/.../Title^abc1234` so duplicate
    // titles in the same parent chain still resolve uniquely.
    #[test]
    fn parse_link_nested_with_short_id() {
        assert_eq!(
            parse_link("Jehu/Folder/Untitledkkk^abc12345"),
            Some(LinkForm::Nested {
                project: "Jehu".into(),
                parent_path: vec!["Folder".into()],
                title: "Untitledkkk".into(),
                short_id: Some("abc12345".into()),
            })
        );
    }

    #[test]
    fn parse_link_two_segments_with_short_id_becomes_nested_empty_path() {
        // `Project/Title^short` has no parent chain — represented as
        // Nested with parent_path = []. Resolver treats this as
        // "top-level note in Project, narrowed by short_id".
        assert_eq!(
            parse_link("Jehu/Untitledkkk^abc12345"),
            Some(LinkForm::Nested {
                project: "Jehu".into(),
                parent_path: Vec::new(),
                title: "Untitledkkk".into(),
                short_id: Some("abc12345".into()),
            })
        );
    }
}
