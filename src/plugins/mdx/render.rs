//! MDX renderer. Reuses the markdown renderer for `MdxNode::Markdown` nodes; emits a
//! styled escaped code block for `JsxBlock` — explicit non-goal: do not evaluate JSX.

use dioxus::prelude::*;

use super::parser::MdxNode;
use crate::plugins::markdown::render::render_node;

#[component]
pub fn MdxView(nodes: Vec<MdxNode>) -> Element {
    rsx! {
        article { class: "markdown-prose mdx-prose",
            for node in nodes.iter() {
                {render_mdx_node(node)}
            }
        }
    }
}

fn render_mdx_node(node: &MdxNode) -> Element {
    match node {
        MdxNode::Markdown(md) => render_node(md),
        MdxNode::JsxBlock { raw } => rsx! {
            pre { class: "mdx-jsx-block",
                code { "{raw}" }
            }
        },
    }
}
