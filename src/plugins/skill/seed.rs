//! SDLC seed skills, embedded into the binary at compile time.
//!
//! The 15 skill files plus README under `seed-skills-updated/` ship
//! with every operon build via `include_dir!`. The `install_seed_skills`
//! MCP tool and the explorer "+" project flow both pull from here, so
//! a shipped binary can populate a new project with the cascade chain
//! without the user manually pointing at a source folder.
//!
//! Editing a file under `seed-skills-updated/` triggers a Cargo rerun
//! (see `build.rs`), so iterating on a seed prompt and `r`-rebuilding
//! the dev binary picks up the new body.

use include_dir::{include_dir, Dir};

/// Compile-time embedded copy of the `seed-skills-updated/` tree.
/// Includes every `.md` file at the directory root (README + the
/// numbered skills).
pub static SEED_SKILLS: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/seed-skills-updated");

/// One seed-skill entry. `stem` is the filename without the `.md`
/// extension (e.g. `02-ba-discover-epics`) and serves as both the note
/// title and the on-disk slug. `body` is the raw file content.
#[derive(Debug, Clone, Copy)]
pub struct SeedSkill {
    pub stem: &'static str,
    pub body: &'static str,
}

/// Iterate every embedded seed skill except the README. Order matches
/// the directory listing — file names sort lexically by their numeric
/// prefix (`00-`, `02-`, `02n-`, …), which is the cascade pipeline
/// order, so callers can install in that order without resorting.
pub fn seed_skill_list() -> impl Iterator<Item = SeedSkill> {
    let mut entries: Vec<&include_dir::File<'static>> = SEED_SKILLS
        .files()
        .filter(|f| {
            f.path().extension().and_then(|e| e.to_str()) == Some("md")
                && !f
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("readme"))
                    .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|f| f.path().to_path_buf());
    entries.into_iter().filter_map(|f| {
        let stem = f.path().file_stem().and_then(|s| s.to_str())?;
        let body = f.contents_utf8()?;
        Some(SeedSkill { stem, body })
    })
}

/// README body shipped alongside the skills, used as the prose preamble
/// for the auto-generated `SKILLS` index note. Returns `None` if the
/// seed bundle ships without a README (it does today, but the lookup
/// is defensive in case it's removed).
pub fn seed_readme() -> Option<&'static str> {
    SEED_SKILLS
        .files()
        .find(|f| {
            f.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("readme"))
                .unwrap_or(false)
                && f.path().extension().and_then(|e| e.to_str()) == Some("md")
        })
        .and_then(|f| f.contents_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_skill_list_is_non_empty_and_skips_readme() {
        let stems: Vec<&'static str> = seed_skill_list().map(|s| s.stem).collect();
        assert!(
            stems.len() >= 10,
            "expected the cascade chain, got {} skills",
            stems.len()
        );
        assert!(
            stems.iter().all(|s| !s.eq_ignore_ascii_case("readme")),
            "README slipped into the seed-skill iterator: {stems:?}"
        );
    }

    #[test]
    fn seed_skill_bodies_are_non_empty() {
        for skill in seed_skill_list() {
            assert!(
                !skill.body.trim().is_empty(),
                "seed skill {} has an empty body",
                skill.stem
            );
        }
    }

    #[test]
    fn seed_skill_list_is_sorted() {
        let stems: Vec<&'static str> = seed_skill_list().map(|s| s.stem).collect();
        let mut sorted = stems.clone();
        sorted.sort();
        assert_eq!(
            stems, sorted,
            "seed_skill_list should iterate in stem-sorted (cascade) order"
        );
    }
}
