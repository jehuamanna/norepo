//! Render an [`MdNode`] tree into Dioxus RSX, themed via Phase-1 token CSS variables.

use dioxus::prelude::*;

use super::nodes::MdNode;
use super::parser;

#[component]
pub fn MarkdownView(content: String) -> Element {
    let nodes = parser::parse(&content);

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
