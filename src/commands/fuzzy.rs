//! Lightweight fuzzy subsequence scorer used by the command palette.
//!
//! Scoring rules:
//! - +10 per matched character.
//! - +5 bonus when a match lands at index 0 or right after a separator (` `, `.`, `_`, `-`).
//! - +2 bonus when matches are contiguous in the candidate.
//! - -1 per gap (skipped candidate character between matches).
//!
//! Matching is case-insensitive. An empty query returns `Some(0)`. A query that is not a
//! subsequence of the candidate returns `None`.

pub fn score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let c: Vec<char> = candidate.chars().flat_map(char::to_lowercase).collect();
    let mut score: i32 = 0;
    let mut qi = 0;
    let mut last_match: Option<usize> = None;
    for (ci, ch) in c.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if *ch == q[qi] {
            score += 10;
            let after_separator =
                ci == 0 || matches!(c[ci - 1], ' ' | '.' | '_' | '-');
            if after_separator {
                score += 5;
            }
            if last_match.map(|prev| prev + 1 == ci).unwrap_or(false) {
                score += 2;
            }
            last_match = Some(ci);
            qi += 1;
        } else if last_match.is_some() {
            score -= 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_match_succeeds() {
        assert!(score("xyz", "abxyz").is_some());
    }

    #[test]
    fn non_subsequence_returns_none() {
        assert!(score("zyx", "abxyz").is_none());
    }

    #[test]
    fn theme_in_toggle_theme_outranks_unrelated() {
        let s1 = score("the", "Toggle Theme").unwrap();
        let s2 = score("the", "Other widget").unwrap();
        assert!(s1 > s2, "{s1} should be > {s2}");
    }

    #[test]
    fn empty_query_returns_zero() {
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn case_insensitive_match() {
        assert!(score("THEME", "toggle theme").is_some());
    }

    #[test]
    fn boundary_bonus_outranks_mid_word() {
        let s1 = score("tt", "Toggle Theme").unwrap();
        let s2 = score("tt", "atatat").unwrap();
        assert!(s1 > s2, "{s1} should be > {s2}");
    }
}
