//! Post-process pass that converts `[[...]]` and `![[...]]` patterns inside
//! `MdNode::Text` nodes into `MdNode::WikiLink` nodes.
//!
//! Plans-Phase-5-vfs-wikilinks. Pure function over the AST so the existing
//! pulldown-cmark parser stays untouched and tests can target `expand_wiki`
//! in isolation. Inline-code and code-block nodes are not visited (they
//! emit their own `Code` / `CodeBlock` variants which we leave alone), so
//! `[[X]]` literals inside fenced code blocks render verbatim.

use super::nodes::MdNode;

/// Walk the node tree and split any `MdNode::Text` containing wikilink
/// patterns into a sequence of `Text` + `WikiLink` siblings.
pub fn expand_wiki(nodes: Vec<MdNode>) -> Vec<MdNode> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        match n {
            MdNode::Text(t) => out.extend(split_text(&t)),
            MdNode::Heading { level, children } => out.push(MdNode::Heading {
                level,
                children: expand_wiki(children),
            }),
            MdNode::Paragraph { children } => out.push(MdNode::Paragraph {
                children: expand_wiki(children),
            }),
            MdNode::Strong(c) => out.push(MdNode::Strong(expand_wiki(c))),
            MdNode::Emphasis(c) => out.push(MdNode::Emphasis(expand_wiki(c))),
            MdNode::Link {
                dest,
                title,
                children,
            } => out.push(MdNode::Link {
                dest,
                title,
                children: expand_wiki(children),
            }),
            MdNode::BlockQuote(c) => out.push(MdNode::BlockQuote(expand_wiki(c))),
            MdNode::List { ordered, items } => out.push(MdNode::List {
                ordered,
                items: items.into_iter().map(expand_wiki).collect(),
            }),
            MdNode::ListItem(c) => out.push(MdNode::ListItem(expand_wiki(c))),
            // Code / CodeBlock / Image / Rule / WikiLink pass through.
            n => out.push(n),
        }
    }
    out
}

fn split_text(s: &str) -> Vec<MdNode> {
    let mut out: Vec<MdNode> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    while i + 1 < bytes.len() {
        // Detect `[[` or `![[`.
        let (open_len, embed) = if bytes[i] == b'!' && bytes[i + 1] == b'[' && i + 2 < bytes.len() && bytes[i + 2] == b'[' {
            (3, true)
        } else if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            (2, false)
        } else {
            i += 1;
            continue;
        };
        // Find matching `]]`.
        let inner_start = i + open_len;
        if let Some(rel_close) = s[inner_start..].find("]]") {
            let inner = &s[inner_start..inner_start + rel_close];
            // Reject if the inner contains a newline (prevents pathological
            // matches across lines / paragraphs).
            if inner.contains('\n') {
                i += 1;
                continue;
            }
            // Emit the literal text before the wikilink, if any.
            if text_start < i {
                out.push(MdNode::Text(s[text_start..i].to_string()));
            }
            out.push(MdNode::WikiLink {
                target: inner.to_string(),
                embed,
            });
            i = inner_start + rel_close + 2;
            text_start = i;
            continue;
        }
        // No matching `]]`; advance past this `[`.
        i += 1;
    }
    // Flush the trailing literal segment.
    if text_start < s.len() {
        out.push(MdNode::Text(s[text_start..].to_string()));
    }
    if out.is_empty() {
        // String was empty — preserve it so callers that count Text nodes
        // don't get fooled.
        out.push(MdNode::Text(String::new()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_unchanged() {
        let r = split_text("hello world");
        assert_eq!(r, vec![MdNode::Text("hello world".into())]);
    }

    #[test]
    fn single_wikilink() {
        let r = split_text("see [[Other Note]] for context");
        assert_eq!(
            r,
            vec![
                MdNode::Text("see ".into()),
                MdNode::WikiLink {
                    target: "Other Note".into(),
                    embed: false,
                },
                MdNode::Text(" for context".into()),
            ]
        );
    }

    #[test]
    fn embed_wikilink() {
        let r = split_text("here: ![[Image Note^abc]] yes");
        assert_eq!(
            r,
            vec![
                MdNode::Text("here: ".into()),
                MdNode::WikiLink {
                    target: "Image Note^abc".into(),
                    embed: true,
                },
                MdNode::Text(" yes".into()),
            ]
        );
    }

    #[test]
    fn project_path_form() {
        let r = split_text("[[Project/Saving]]");
        assert_eq!(
            r,
            vec![MdNode::WikiLink {
                target: "Project/Saving".into(),
                embed: false,
            }]
        );
    }

    #[test]
    fn unmatched_open_is_literal() {
        let r = split_text("text [[ unclosed");
        // No `]]` — fall through; emits the whole string as text.
        assert_eq!(r, vec![MdNode::Text("text [[ unclosed".into())]);
    }

    #[test]
    fn newline_inside_does_not_match() {
        let r = split_text("a [[multi\nline]] b");
        assert_eq!(r, vec![MdNode::Text("a [[multi\nline]] b".into())]);
    }

    #[test]
    fn multiple_wikilinks_in_one_text() {
        let r = split_text("see [[A]] and [[B]] please");
        assert_eq!(
            r,
            vec![
                MdNode::Text("see ".into()),
                MdNode::WikiLink {
                    target: "A".into(),
                    embed: false,
                },
                MdNode::Text(" and ".into()),
                MdNode::WikiLink {
                    target: "B".into(),
                    embed: false,
                },
                MdNode::Text(" please".into()),
            ]
        );
    }

    #[test]
    fn expand_wiki_skips_code() {
        // Inline code arrives as MdNode::Code, not Text — so [[X]] inside
        // code is preserved literally.
        let input = vec![
            MdNode::Paragraph {
                children: vec![
                    MdNode::Text("plain ".into()),
                    MdNode::Code("[[X]]".into()),
                    MdNode::Text(" then [[Y]] end".into()),
                ],
            },
        ];
        let out = expand_wiki(input);
        assert_eq!(
            out,
            vec![MdNode::Paragraph {
                children: vec![
                    MdNode::Text("plain ".into()),
                    MdNode::Code("[[X]]".into()),
                    MdNode::Text(" then ".into()),
                    MdNode::WikiLink {
                        target: "Y".into(),
                        embed: false
                    },
                    MdNode::Text(" end".into()),
                ],
            }]
        );
    }
}
