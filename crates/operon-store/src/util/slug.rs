//! Slug derivation for artifact notes' on-disk paths.
//!
//! `slugify` produces a filesystem-friendly lowercased-ascii-with-dashes
//! token. `unique_slug` resolves sibling collisions by appending `-2`, `-3`, …

pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_dash = true;
    for ch in title.chars() {
        let mapped: Option<char> = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if ch.is_alphanumeric() {
            // Non-ASCII alphanumeric (e.g. accented chars) → dash. ASCII-only
            // keeps cross-platform path behavior predictable.
            None
        } else {
            None
        };
        match mapped {
            Some(c) => {
                out.push(c);
                last_dash = false;
            }
            None => {
                if !last_dash {
                    out.push('-');
                    last_dash = true;
                }
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

pub fn unique_slug(base: &str, existing: &[&str]) -> String {
    if !existing.iter().any(|e| *e == base) {
        return base.to_string();
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !existing.iter().any(|e| *e == candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basics() {
        assert_eq!(slugify("CE inputs"), "ce-inputs");
        assert_eq!(slugify("epic-01-playable-memory-match"), "epic-01-playable-memory-match");
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("   leading & trailing   "), "leading-trailing");
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
        assert_eq!(slugify("a__b__c"), "a-b-c");
    }

    #[test]
    fn unique_slug_collisions() {
        assert_eq!(unique_slug("discovery", &[]), "discovery");
        assert_eq!(unique_slug("discovery", &["discovery"]), "discovery-2");
        assert_eq!(
            unique_slug("discovery", &["discovery", "discovery-2"]),
            "discovery-3"
        );
        assert_eq!(
            unique_slug("discovery", &["discovery", "discovery-3"]),
            "discovery-2"
        );
    }
}
