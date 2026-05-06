//! Post-process pass that converts `[[...]]` and `![[...]]` patterns inside
//! `MdNode::Text` nodes into `MdNode::WikiLink` nodes — and a body-level
//! scanner used by the save pipeline to extract a flat (target, embed) list
//! for graph rebuilding.
//!
//! Plans-Phase-5-vfs-wikilinks. Pure function over the AST so the existing
//! pulldown-cmark parser stays untouched and tests can target `expand_wiki`
//! in isolation. Inline-code and code-block nodes are not visited (they
//! emit their own `Code` / `CodeBlock` variants which we leave alone), so
//! `[[X]]` literals inside fenced code blocks render verbatim.

use super::nodes::MdNode;

/// One extracted reference. `target` is the inner text (no `[[` / `]]`),
/// `embed` is true for `![[…]]`. Suitable for `LinkRow.target_text` /
/// `LinkRow.is_embed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedLink {
    pub target: String,
    pub embed: bool,
}

/// Body-level scanner: walks a markdown body (including raw `[[…]]`
/// patterns) and emits every wikilink occurrence in document order.
/// Skips fenced code blocks and inline code spans so `[[X]]` typed inside
/// them stays literal. Repeated targets are kept (caller dedupes).
pub fn extract_links(body: &str) -> Vec<ExtractedLink> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut in_fence = false;
    let mut at_line_start = true;

    while i < bytes.len() {
        // Detect a fenced code-block toggle on a line that starts with
        // ``` (three backticks). Tilde fences are uncommon in our notes;
        // skip for v1.
        if at_line_start && i + 3 <= bytes.len() && &bytes[i..i + 3] == b"```" {
            in_fence = !in_fence;
            // Skip to end of line.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'\n' {
            at_line_start = true;
            i += 1;
            continue;
        }
        at_line_start = false;
        if in_fence {
            i += 1;
            continue;
        }
        // Inline code span: skip until closing backtick on the same line.
        if bytes[i] == b'`' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'`' && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            continue;
        }
        // Wikilink: `[[…]]` or `![[…]]`.
        let (open_len, embed) =
            if bytes[i] == b'!' && i + 2 < bytes.len() && bytes[i + 1] == b'[' && bytes[i + 2] == b'[' {
                (3, true)
            } else if bytes[i] == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                (2, false)
            } else {
                i += 1;
                continue;
            };
        let inner_start = i + open_len;
        if let Some(rel_close) = body[inner_start..].find("]]") {
            let inner = &body[inner_start..inner_start + rel_close];
            if !inner.contains('\n') {
                out.push(ExtractedLink {
                    target: inner.to_string(),
                    embed,
                });
                i = inner_start + rel_close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

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
    fn extract_links_finds_targets_in_order() {
        let body = "see [[A]] and ![[B^abc]] then [[C/D]] end";
        let got = extract_links(body);
        assert_eq!(
            got,
            vec![
                ExtractedLink {
                    target: "A".into(),
                    embed: false
                },
                ExtractedLink {
                    target: "B^abc".into(),
                    embed: true
                },
                ExtractedLink {
                    target: "C/D".into(),
                    embed: false
                },
            ]
        );
    }

    #[test]
    fn extract_links_skips_fenced_code() {
        let body = "before\n```\n[[NOT_A_LINK]]\n```\n[[YES]]\n";
        let got = extract_links(body);
        assert_eq!(
            got,
            vec![ExtractedLink {
                target: "YES".into(),
                embed: false
            }]
        );
    }

    #[test]
    fn extract_links_skips_inline_code() {
        let body = "code: `[[NOT]]` real: [[YES]] end";
        let got = extract_links(body);
        assert_eq!(
            got,
            vec![ExtractedLink {
                target: "YES".into(),
                embed: false
            }]
        );
    }

    #[test]
    fn extract_links_ignores_unmatched_open() {
        let body = "trailing [[ unclosed";
        let got = extract_links(body);
        assert!(got.is_empty());
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
