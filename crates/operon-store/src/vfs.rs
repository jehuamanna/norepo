//! Virtual File System helpers shared between the explorer breadcrumb,
//! Plans-Phase-5 wikilink resolver, and any future tooling that needs to
//! map note paths or wikilink text to canonical `NoteId`s.

use uuid::Uuid;

use crate::error::StoreError;
use crate::repos::{LocalNoteRepository, LocalProjectRepository};

/// Parsed wikilink target. Three forms are supported, mirroring the seed:
///
/// - `[[Note]]`                 → `Relative { title }` (resolves against the
///   source note's project).
/// - `[[Project/Note]]`         → `Absolute { project, title }`.
/// - `[[Note^abc12345]]`        → `Disambiguated { title, short_id }` — the
///   short id is at least 8 chars of the canonical UUID hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkForm {
    Relative {
        title: String,
    },
    Absolute {
        project: String,
        title: String,
    },
    Disambiguated {
        title: String,
        short_id: String,
    },
}

/// Parse the inner text of a wikilink (the part between `[[` and `]]`)
/// into one of the three forms. Returns `None` for empty/whitespace-only
/// text.
pub fn parse_link(text: &str) -> Option<LinkForm> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((title, short_id)) = trimmed.rsplit_once('^') {
        let title = title.trim().to_string();
        let short_id = short_id.trim().to_lowercase();
        if !title.is_empty() && short_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(LinkForm::Disambiguated { title, short_id });
        }
    }
    if let Some((project, title)) = trimmed.split_once('/') {
        return Some(LinkForm::Absolute {
            project: project.trim().to_string(),
            title: title.trim().to_string(),
        });
    }
    Some(LinkForm::Relative {
        title: trimmed.to_string(),
    })
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
}
