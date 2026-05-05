//! Regression guard: every pictograph / emoji used in pre-Icon-component shell call sites
//! must stay out of `src/`. New pictographs would defeat the colourless-icon work
//! (Plans-Phase-4-colourless-icons).
//!
//! Path-prefix exclusions:
//! - `src/theme/palettes/`: contains hex colour strings; no glyph use, but excluded as a
//!   defensive measure since palette files may reference Unicode names in future.
//! - This test file itself: contains the banned glyphs as test fixtures.

use std::fs;
use std::path::{Path, PathBuf};

const BANNED_GLYPHS: &[char] = &[
    '\u{25BC}', // ▼
    '\u{25BE}', // ▾
    '\u{25B6}', // ▶
    '\u{25B8}', // ▸
    '\u{25E7}', // ◧
    '\u{00D7}', // ×
    '\u{25CF}', // ●
    '\u{1F4DA}', // 📚
    '\u{25C0}', // ◀
    '\u{25C1}', // ◁
];

/// Recursively walk `dir`, collecting every regular `.rs` file path that's not under an
/// excluded prefix. The `excluded` paths are matched against the entry path's start.
fn rs_files(dir: &Path, excluded: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if excluded.iter().any(|ex| p.starts_with(ex)) {
            continue;
        }
        if p.is_dir() {
            out.extend(rs_files(&p, excluded));
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
    out
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn src_tree_has_no_pictographs_outside_palettes() {
    let root = manifest_dir().join("src");
    let excluded = [manifest_dir().join("src/theme/palettes")];
    let files = rs_files(&root, &excluded);
    assert!(!files.is_empty(), "no .rs files found under src/");

    let mut violations = Vec::new();
    for path in files {
        let body = match fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for &g in BANNED_GLYPHS {
            if body.contains(g) {
                violations.push(format!("{}: {:?}", path.display(), g));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "pictograph icons leaked back into src/:\n{}",
        violations.join("\n")
    );
}

#[test]
fn header_svg_uses_only_currentcolor_or_none() {
    let path = manifest_dir().join("assets/header.svg");
    let body = fs::read_to_string(&path).expect("assets/header.svg readable");

    let bad = [
        " fill=\"#",
        " stroke=\"#",
        "fill='#",
        "stroke='#",
    ];
    for needle in bad {
        assert!(
            !body.contains(needle),
            "assets/header.svg still contains a hard-coded colour ({needle:?})"
        );
    }

    // Sanity: at least one currentColor reference exists, so the SVG is actually theme-aware.
    assert!(
        body.contains("currentColor"),
        "assets/header.svg has no currentColor references"
    );
}
