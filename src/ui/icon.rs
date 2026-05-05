//! Inline-SVG `Icon` component backed by a static path registry.
//!
//! Every shipped icon shares a 16x16 viewBox (matches VSCode codicons). Path data is
//! `currentColor`-fillable; the parent's CSS `color` decides the actual tint.

use dioxus::prelude::*;

/// Lookup the SVG path data for a named icon. Returns a placeholder square for unknown
/// names so the missing icon is visible in dev without crashing the render tree.
pub fn lookup(name: &str) -> &'static str {
    for &(n, p) in PATHS {
        if n == name {
            return p;
        }
    }
    PLACEHOLDER_PATH
}

/// True iff the registry recognises `name`.
pub fn has(name: &str) -> bool {
    PATHS.iter().any(|(n, _)| *n == name)
}

const PLACEHOLDER_PATH: &str = "M2 2H14V14H2V2Z";

/// Static registry. Hand-authored 16x16 paths — keep entries alphabetical to ease review.
const PATHS: &[(&str, &str)] = &[
    // Outlined book — Notes Explorer activity icon.
    (
        "book",
        "M3 2v12c0-.6.4-1 1-1h9V2H4c-.6 0-1 .4-1 1zM4 14a1 1 0 0 0 1 1h8v-1H5z",
    ),
    // Filled circle — tab dirty marker.
    ("circle-dot", "M8 4a4 4 0 1 0 0 8 4 4 0 0 0 0-8z"),
    ("chevron-down", "M3.5 6.5L8 11l4.5-4.5-1-1L8 9 4.5 5.5z"),
    ("chevron-left", "M9.5 12.5L5 8l4.5-4.5 1 1L7 8l3.5 3.5z"),
    ("chevron-right", "M6.5 12.5L11 8 6.5 3.5l-1 1L9 8l-3.5 3.5z"),
    ("chevron-up", "M3.5 9.5L8 5l4.5 4.5-1 1L8 7l-3.5 3.5z"),
    // Hollow rectangle with a right-third splitter — companion-area toggle.
    (
        "sidebar-right",
        "M2 3h12v10H2V3zm1 1v8h6V4H3zm7 0v8h3V4h-3z",
    ),
    // Hollow square outline — sidebar expanded indicator.
    (
        "square",
        "M3 3h10v10H3V3zm1 1v8h8V4H4z",
    ),
    // Hollow rectangle with a bottom-third splitter — panel toggle.
    (
        "panel",
        "M2 3h12v10H2V3zm1 1v6h10V4H3zm0 7v2h10v-2H3z",
    ),
    // Thin diagonal cross — tab close affordance.
    (
        "x",
        "M4.7 4l-.7.7L7.3 8 4 11.3l.7.7L8 8.7 11.3 12l.7-.7L8.7 8 12 4.7 11.3 4 8 7.3z",
    ),
];

#[derive(Clone, PartialEq, Props)]
pub struct IconProps {
    /// Icon name from the registry (`chevron-down`, `book`, `x`, …).
    pub name: String,
    /// Width / height in CSS pixels. Defaults to 16 to match VSCode codicons.
    #[props(default = 16)]
    pub size: u16,
    /// If set, an `<svg><title>` is emitted (for screen readers); `aria-hidden` flips to
    /// `false`. Without `title`, the icon is treated as decorative.
    #[props(default)]
    pub title: Option<String>,
    /// Extra CSS class merged with the default `operon-icon`.
    #[props(default)]
    pub class: Option<String>,
}

#[component]
pub fn Icon(props: IconProps) -> Element {
    let path = lookup(&props.name);
    let class = match &props.class {
        Some(extra) => format!("operon-icon {extra}"),
        None => "operon-icon".to_string(),
    };
    let aria_hidden = if props.title.is_some() { "false" } else { "true" };
    let size_str = props.size.to_string();

    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            width: "{size_str}",
            height: "{size_str}",
            view_box: "0 0 16 16",
            fill: "currentColor",
            "aria-hidden": "{aria_hidden}",
            class: "{class}",
            "data-icon": "{props.name}",
            if let Some(t) = &props.title { title { "{t}" } }
            path { d: "{path}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_known_names_returns_non_empty_path() {
        for &(name, _) in PATHS {
            let p = lookup(name);
            assert!(!p.is_empty(), "{name} returned empty path");
            assert!(p.starts_with('M'), "{name} path should start with a Moveto");
        }
    }

    #[test]
    fn lookup_unknown_name_returns_placeholder() {
        assert_eq!(lookup("not-a-real-icon"), PLACEHOLDER_PATH);
        assert_eq!(lookup(""), PLACEHOLDER_PATH);
    }

    #[test]
    fn has_known_name() {
        assert!(has("chevron-down"));
        assert!(has("book"));
        assert!(has("x"));
    }

    #[test]
    fn has_unknown_name_false() {
        assert!(!has("nope"));
    }

    #[test]
    fn registry_includes_every_call_site_name() {
        // The shell call sites swapped from glyphs to these names. If a name disappears
        // from PATHS, this fires before the call sites build-fail.
        for required in [
            "chevron-down",
            "chevron-right",
            "chevron-up",
            "chevron-left",
            "square",
            "x",
            "circle-dot",
            "book",
            "panel",
            "sidebar-right",
        ] {
            assert!(has(required), "missing icon: {required}");
        }
    }
}
