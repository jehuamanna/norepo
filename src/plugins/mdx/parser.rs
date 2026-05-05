//! MDX parser. Splits input into top-level JSX blocks + markdown chunks; markdown chunks
//! are fed to the existing pulldown-cmark-based parser.
//!
//! Top-level JSX detection uses a conservative regex-style heuristic: a line that begins
//! with `<TagName` (uppercase first letter) up to a matching close tag (or self-closing
//! `/>`). False negatives in pathological cases (multi-line JSX with blank lines inside)
//! are acceptable for v1; the safer choice is to occasionally render JSX as markdown rather
//! than to truncate or mis-bound a paragraph.
//!
//! Import statements (`import X from "...";`) are also captured as JsxBlocks so they don't
//! leak into the rendered prose.

use crate::plugins::markdown::{nodes::MdNode, parser};

#[derive(Clone, Debug, PartialEq)]
pub enum MdxNode {
    Markdown(MdNode),
    JsxBlock { raw: String },
}

pub fn parse_mdx(input: &str) -> Vec<MdxNode> {
    // Strip frontmatter the same way the markdown parser does — pulldown-cmark sees the
    // post-frontmatter text. The inner parser also strips frontmatter, but doing it here
    // means our chunk-walking sees the content the user expects.
    let body = strip_frontmatter(input);

    let mut out = Vec::new();
    let mut markdown_buf = String::new();

    for line in body.split_inclusive('\n') {
        if is_top_level_jsx_or_import(line) {
            // Flush any accumulated markdown chunk first.
            if !markdown_buf.is_empty() {
                let nodes = parser::parse(&markdown_buf);
                for n in nodes {
                    out.push(MdxNode::Markdown(n));
                }
                markdown_buf.clear();
            }
            out.push(MdxNode::JsxBlock { raw: line.trim_end_matches('\n').to_string() });
        } else {
            markdown_buf.push_str(line);
        }
    }

    if !markdown_buf.is_empty() {
        let nodes = parser::parse(&markdown_buf);
        for n in nodes {
            out.push(MdxNode::Markdown(n));
        }
    }

    out
}

fn strip_frontmatter(input: &str) -> &str {
    if !input.starts_with("---\n") && !input.starts_with("---\r\n") {
        return input;
    }
    let after_open = &input[4..];
    if let Some(end_idx) = after_open.find("\n---\n") {
        return &after_open[end_idx + 5..];
    }
    if let Some(end_idx) = after_open.find("\r\n---\r\n") {
        return &after_open[end_idx + 7..];
    }
    input
}

/// Returns true if a line is plausibly a top-level JSX or ESM import statement.
fn is_top_level_jsx_or_import(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
        return true;
    }
    // JSX: `<TagName...` where TagName starts with an uppercase ASCII letter.
    let bytes = trimmed.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'<' {
        return false;
    }
    bytes[1].is_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jsx_block(nodes: &[MdxNode]) -> Option<&str> {
        nodes.iter().find_map(|n| match n {
            MdxNode::JsxBlock { raw } => Some(raw.as_str()),
            _ => None,
        })
    }

    #[test]
    fn detects_self_closing_jsx_block() {
        let nodes = parse_mdx("# Title\n\n<Foo bar=\"x\" />\n");
        assert!(jsx_block(&nodes).is_some());
        assert!(nodes.iter().any(|n| matches!(n,
            MdxNode::Markdown(MdNode::Heading { level: 1, .. }))));
    }

    #[test]
    fn detects_import_statement_as_jsx_block() {
        let nodes = parse_mdx("import X from \"./y\";\n# Title\n");
        assert!(jsx_block(&nodes).is_some());
    }

    #[test]
    fn does_not_capture_lt_text_as_jsx() {
        let nodes = parse_mdx("a < b is true\n");
        assert!(jsx_block(&nodes).is_none());
        assert!(nodes
            .iter()
            .any(|n| matches!(n, MdxNode::Markdown(MdNode::Paragraph { .. }))));
    }

    #[test]
    fn pure_markdown_has_no_jsx_blocks() {
        let nodes = parse_mdx("# Heading\n\nA paragraph.\n");
        assert!(jsx_block(&nodes).is_none());
    }

    #[test]
    fn frontmatter_stripped_in_mdx() {
        let nodes = parse_mdx("---\ntitle: x\n---\n\n# H\n");
        let texts: Vec<String> = nodes
            .iter()
            .filter_map(|n| match n {
                MdxNode::Markdown(MdNode::Heading { children, .. }) => Some(
                    children
                        .iter()
                        .filter_map(|c| match c {
                            MdNode::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "H"));
        let raw_strings: Vec<String> = nodes
            .iter()
            .filter_map(|n| match n {
                MdxNode::JsxBlock { raw } => Some(raw.clone()),
                _ => None,
            })
            .collect();
        assert!(!raw_strings.iter().any(|r| r.contains("title:")));
    }

    #[test]
    fn lowercase_tag_not_captured_as_jsx() {
        // <span> is HTML; pulldown-cmark drops raw HTML, so <span> in a paragraph
        // becomes invisible — but it shouldn't be captured as a JSX block (we only
        // capture uppercase-leading tags).
        let nodes = parse_mdx("<span>plain</span>\n");
        assert!(jsx_block(&nodes).is_none());
    }
}
