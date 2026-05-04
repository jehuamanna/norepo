//! Convert pulldown-cmark events into a typed [`MdNode`] tree.
//!
//! Raw HTML events are dropped (no execution, no display). YAML-style frontmatter at the
//! top of the document is removed before parsing so seed-prompt notes look clean.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag};

use super::nodes::MdNode;

/// Strip a leading `---\n...\n---\n` block. Anything else is returned unchanged.
pub fn strip_frontmatter(input: &str) -> &str {
    let Some(rest) = input.strip_prefix("---\n") else {
        return input;
    };
    let Some(end) = rest.find("\n---") else {
        return input; // no end fence; treat whole input as body
    };
    let after = end + "\n---".len();
    let tail = &rest[after..];
    if tail.is_empty() {
        return tail;
    }
    if let Some(stripped) = tail.strip_prefix('\n') {
        return stripped;
    }
    // Closing fence not followed by newline; abort frontmatter strip.
    input
}

/// Walk pulldown-cmark events into an [`MdNode`] tree.
pub fn parse(input: &str) -> Vec<MdNode> {
    let body = strip_frontmatter(input);
    let parser = Parser::new(body);

    let mut stack: Vec<Vec<MdNode>> = vec![Vec::new()];
    let mut tag_stack: Vec<OpenTag> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                let opened = OpenTag::from_tag(&tag);
                stack.push(Vec::new());
                tag_stack.push(opened);
            }
            Event::End(_) => {
                let children = stack.pop().expect("balanced");
                if let Some(open) = tag_stack.pop() {
                    if let Some(node) = build_node(open, children) {
                        stack.last_mut().expect("non-empty").push(node);
                    }
                }
            }
            Event::Text(t) => {
                stack.last_mut().expect("non-empty").push(MdNode::Text(t.into_string()));
            }
            Event::Code(t) => {
                stack.last_mut().expect("non-empty").push(MdNode::Code(t.into_string()));
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                // dropped
            }
            Event::SoftBreak => {
                stack.last_mut().expect("non-empty").push(MdNode::Text(" ".into()));
            }
            Event::HardBreak => {
                stack.last_mut().expect("non-empty").push(MdNode::Text("\n".into()));
            }
            Event::Rule => {
                stack.last_mut().expect("non-empty").push(MdNode::Rule);
            }
            _ => {}
        }
    }

    stack.into_iter().next().unwrap_or_default()
}

/// A tag plus the data we need to build the corresponding [`MdNode`] when it closes.
enum OpenTag {
    Heading(u8),
    Paragraph,
    Strong,
    Emphasis,
    Link { dest: String, title: String },
    Image { dest: String, _title: String },
    CodeBlock { lang: Option<String> },
    BlockQuote,
    List { ordered: bool },
    Item,
    Other,
}

impl OpenTag {
    fn from_tag(tag: &Tag<'_>) -> Self {
        match tag {
            Tag::Heading { level, .. } => OpenTag::Heading(heading_level_to_u8(*level)),
            Tag::Paragraph => OpenTag::Paragraph,
            Tag::Strong => OpenTag::Strong,
            Tag::Emphasis => OpenTag::Emphasis,
            Tag::Link { dest_url, title, .. } => OpenTag::Link {
                dest: dest_url.to_string(),
                title: title.to_string(),
            },
            Tag::Image { dest_url, title, .. } => OpenTag::Image {
                dest: dest_url.to_string(),
                _title: title.to_string(),
            },
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(s) if !s.is_empty() => Some(s.to_string()),
                    _ => None,
                };
                OpenTag::CodeBlock { lang }
            }
            Tag::BlockQuote => OpenTag::BlockQuote,
            Tag::List(start) => OpenTag::List { ordered: start.is_some() },
            Tag::Item => OpenTag::Item,
            _ => OpenTag::Other,
        }
    }
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn build_node(open: OpenTag, children: Vec<MdNode>) -> Option<MdNode> {
    match open {
        OpenTag::Heading(level) => Some(MdNode::Heading { level, children }),
        OpenTag::Paragraph => Some(MdNode::Paragraph { children }),
        OpenTag::Strong => Some(MdNode::Strong(children)),
        OpenTag::Emphasis => Some(MdNode::Emphasis(children)),
        OpenTag::Link { dest, title } => Some(MdNode::Link { dest, title, children }),
        OpenTag::Image { dest, _title } => {
            let alt = collect_text(&children);
            Some(MdNode::Image { dest, alt })
        }
        OpenTag::CodeBlock { lang } => {
            let code = collect_text(&children);
            Some(MdNode::CodeBlock { lang, code })
        }
        OpenTag::BlockQuote => Some(MdNode::BlockQuote(children)),
        OpenTag::List { ordered } => {
            let items: Vec<Vec<MdNode>> = children
                .into_iter()
                .filter_map(|n| match n {
                    MdNode::ListItem(c) => Some(c),
                    _ => None,
                })
                .collect();
            Some(MdNode::List { ordered, items })
        }
        OpenTag::Item => Some(MdNode::ListItem(children)),
        OpenTag::Other => None,
    }
}

fn collect_text(nodes: &[MdNode]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            MdNode::Text(s) => out.push_str(s),
            MdNode::Code(s) => out.push_str(s),
            MdNode::Strong(c) | MdNode::Emphasis(c) => out.push_str(&collect_text(c)),
            MdNode::Paragraph { children } => out.push_str(&collect_text(children)),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_para_children(nodes: &[MdNode]) -> &[MdNode] {
        match nodes.first() {
            Some(MdNode::Paragraph { children }) => children,
            _ => panic!("expected paragraph at index 0; got {:?}", nodes),
        }
    }

    #[test]
    fn heading_levels_one_through_six() {
        for n in 1u8..=6 {
            let hashes = "#".repeat(n as usize);
            let src = format!("{hashes} H{n}");
            let nodes = parse(&src);
            match nodes.first() {
                Some(MdNode::Heading { level, children }) => {
                    assert_eq!(*level, n);
                    let txt = collect_text(children);
                    assert!(txt.contains(&format!("H{n}")), "{txt:?}");
                }
                other => panic!("expected heading; got {other:?}"),
            }
        }
    }

    #[test]
    fn paragraph_with_inline_constructs() {
        let nodes = parse("a **b** _c_ `d` [e](https://x)");
        let inline = first_para_children(&nodes);
        let mut saw_strong = false;
        let mut saw_em = false;
        let mut saw_code = false;
        let mut saw_link = false;
        for n in inline {
            match n {
                MdNode::Strong(_) => saw_strong = true,
                MdNode::Emphasis(_) => saw_em = true,
                MdNode::Code(_) => saw_code = true,
                MdNode::Link { dest, .. } => {
                    saw_link = dest == "https://x";
                }
                _ => {}
            }
        }
        assert!(saw_strong && saw_em && saw_code && saw_link);
    }

    #[test]
    fn unordered_list() {
        let nodes = parse("- a\n- **b**");
        match nodes.first() {
            Some(MdNode::List { ordered: false, items }) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected list; got {other:?}"),
        }
    }

    #[test]
    fn ordered_list() {
        let nodes = parse("1. one\n2. two");
        match nodes.first() {
            Some(MdNode::List { ordered: true, items }) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected ordered list; got {other:?}"),
        }
    }

    #[test]
    fn fenced_code_block_with_lang() {
        let nodes = parse("```rust\nfn main(){}\n```");
        let cb = nodes.iter().find(|n| matches!(n, MdNode::CodeBlock { .. })).expect("code block");
        if let MdNode::CodeBlock { lang, code } = cb {
            assert_eq!(lang.as_deref(), Some("rust"));
            assert!(code.contains("fn main"));
        }
    }

    #[test]
    fn block_quote_preserves_inner() {
        let nodes = parse("> quoted\n> still quoted");
        let bq = nodes.iter().find(|n| matches!(n, MdNode::BlockQuote(_))).expect("blockquote");
        if let MdNode::BlockQuote(inner) = bq {
            assert!(!inner.is_empty());
        }
    }

    #[test]
    fn horizontal_rule_present() {
        let nodes = parse("a\n\n---\n\nb");
        assert!(nodes.iter().any(|n| matches!(n, MdNode::Rule)));
    }

    #[test]
    fn image_node() {
        let nodes = parse("![alt](/p.svg)");
        let img = collect_images(&nodes).into_iter().next().expect("image");
        assert_eq!(img.0, "/p.svg");
        assert_eq!(img.1, "alt");
    }

    fn collect_images(nodes: &[MdNode]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for n in nodes {
            match n {
                MdNode::Image { dest, alt } => out.push((dest.clone(), alt.clone())),
                MdNode::Paragraph { children } => out.extend(collect_images(children)),
                MdNode::List { items, .. } => {
                    for item in items {
                        out.extend(collect_images(item));
                    }
                }
                MdNode::BlockQuote(c) => out.extend(collect_images(c)),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn list_inside_blockquote() {
        let nodes = parse("> - a\n> - b");
        let bq = nodes.iter().find(|n| matches!(n, MdNode::BlockQuote(_))).expect("blockquote");
        if let MdNode::BlockQuote(inner) = bq {
            assert!(inner.iter().any(|n| matches!(n, MdNode::List { .. })));
        }
    }

    #[test]
    fn strip_frontmatter_present() {
        let stripped = strip_frontmatter("---\ntitle: x\n---\n# H");
        assert_eq!(stripped, "# H");
    }

    #[test]
    fn strip_frontmatter_no_end_fence() {
        let stripped = strip_frontmatter("---\nno-end\n# H");
        assert_eq!(stripped, "---\nno-end\n# H");
    }

    #[test]
    fn strip_frontmatter_absent() {
        let stripped = strip_frontmatter("# H");
        assert_eq!(stripped, "# H");
    }

    #[test]
    fn strip_frontmatter_empty_input() {
        let stripped = strip_frontmatter("");
        assert_eq!(stripped, "");
    }

    #[test]
    fn raw_html_is_dropped() {
        let nodes = parse("<script>alert(1)</script>\n\nText after");
        let collected = collect_text(&nodes);
        assert!(!collected.contains("<script"));
        assert!(collected.contains("Text after"));
    }
}
