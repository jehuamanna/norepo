//! Render an [`MdNode`] tree into Dioxus RSX, themed via Phase-1 token CSS variables.

use dioxus::prelude::*;

use super::nodes::MdNode;
use super::parser;

#[component]
pub fn MarkdownView(content: String) -> Element {
    // Plans-Phase-5-vfs-wikilinks: post-process the AST to lift `[[…]]` and
    // `![[…]]` patterns out of plain Text nodes into typed `WikiLink` nodes.
    let nodes = super::wikilink::expand_wiki(parser::parse(&content));

    rsx! {
        article {
            class: "markdown-prose",
            for node in nodes.iter() {
                {render_node(node)}
            }
        }
    }
}

pub fn render_node(n: &MdNode) -> Element {
    match n {
        MdNode::Heading { level, children } => match level {
            1 => rsx! { h1 { {render_children(children)} } },
            2 => rsx! { h2 { {render_children(children)} } },
            3 => rsx! { h3 { {render_children(children)} } },
            4 => rsx! { h4 { {render_children(children)} } },
            5 => rsx! { h5 { {render_children(children)} } },
            _ => rsx! { h6 { {render_children(children)} } },
        },
        MdNode::Paragraph { children } => rsx! { p { {render_children(children)} } },
        MdNode::Text(t) => rsx! { "{t}" },
        MdNode::Strong(c) => rsx! { strong { {render_children(c)} } },
        MdNode::Emphasis(c) => rsx! { em { {render_children(c)} } },
        MdNode::Link { dest, children, .. } => rsx! {
            a { href: "{dest}", target: "_blank", {render_children(children)} }
        },
        MdNode::Image { dest, alt } => rsx! { img { src: "{dest}", alt: "{alt}" } },
        MdNode::Code(c) => rsx! { code { class: "md-inline-code", "{c}" } },
        MdNode::CodeBlock { code, .. } => rsx! { pre { code { class: "md-code-block", "{code}" } } },
        MdNode::BlockQuote(c) => rsx! { blockquote { {render_children(c)} } },
        MdNode::List { ordered, items } => {
            if *ordered {
                rsx! {
                    ol {
                        for item in items.iter() {
                            li { {render_children(item)} }
                        }
                    }
                }
            } else {
                rsx! {
                    ul {
                        for item in items.iter() {
                            li { {render_children(item)} }
                        }
                    }
                }
            }
        }
        MdNode::Rule => rsx! { hr {} },
        MdNode::WikiLink { target, embed } => {
            // Plans-Phase-5: target resolution via vfs::resolve_link is a
            // follow-up; for now we render the wikilink as a styled anchor
            // exposing `data-wikilink-target` so the (future) in-app router
            // can intercept clicks. Embed renders as an <img> placeholder
            // until Plans-Phase-6 wires attachment lookup.
            let display = format!("[[{}]]", target);
            let display_embed = format!("![[{}]]", target);
            if *embed {
                rsx! {
                    a {
                        class: "wikilink wikilink-embed",
                        href: "#",
                        "data-wikilink-target": "{target}",
                        "data-wikilink-embed": "true",
                        title: "{target}",
                        "{display_embed}"
                    }
                }
            } else {
                rsx! {
                    a {
                        class: "wikilink",
                        href: "#",
                        "data-wikilink-target": "{target}",
                        title: "{target}",
                        "{display}"
                    }
                }
            }
        }
        MdNode::ListItem(_) => rsx! { "" },
    }
}

fn render_children(children: &[MdNode]) -> Element {
    rsx! {
        for child in children.iter() {
            {render_node(child)}
        }
    }
}
