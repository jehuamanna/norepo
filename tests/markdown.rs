//! Integration tests for the markdown plugin.
//!
//! Parses the comprehensive `Markdown Showcase` fixture (the third sample shipped by
//! `notes_explorer::samples::SAMPLES`) and asserts that every documented construct lands
//! as the expected [`MdNode`] kind.

use operon_dioxus::plugin::PluginRegistry;
use operon_dioxus::plugins::markdown::{nodes::MdNode, parser, MarkdownFormatPlugin};
use operon_dioxus::plugins::notes_explorer::samples::SAMPLES;

fn fixture() -> &'static str {
    SAMPLES
        .iter()
        .find(|(_, title, _)| *title == "Markdown Showcase")
        .expect("showcase sample present")
        .2
}

fn find_node<'a, P>(nodes: &'a [MdNode], pred: &P) -> Option<&'a MdNode>
where
    P: Fn(&MdNode) -> bool,
{
    for n in nodes {
        if pred(n) {
            return Some(n);
        }
        let walked = match n {
            MdNode::Paragraph { children } => find_node(children, pred),
            MdNode::Strong(c) | MdNode::Emphasis(c) | MdNode::BlockQuote(c) => find_node(c, pred),
            MdNode::Heading { children, .. } => find_node(children, pred),
            MdNode::Link { children, .. } => find_node(children, pred),
            MdNode::List { items, .. } => {
                let mut hit = None;
                for item in items {
                    if let Some(h) = find_node(item, pred) {
                        hit = Some(h);
                        break;
                    }
                }
                hit
            }
            _ => None,
        };
        if walked.is_some() {
            return walked;
        }
    }
    None
}

fn collect_all_text(nodes: &[MdNode]) -> String {
    let mut s = String::new();
    for n in nodes {
        match n {
            MdNode::Text(t) | MdNode::Code(t) => s.push_str(t),
            MdNode::CodeBlock { code, .. } => s.push_str(code),
            MdNode::Image { alt, .. } => s.push_str(alt),
            MdNode::Heading { children, .. }
            | MdNode::Paragraph { children }
            | MdNode::Strong(children)
            | MdNode::Emphasis(children)
            | MdNode::BlockQuote(children)
            | MdNode::Link { children, .. } => s.push_str(&collect_all_text(children)),
            MdNode::List { items, .. } => {
                for item in items {
                    s.push_str(&collect_all_text(item));
                }
            }
            _ => {}
        }
    }
    s
}

#[test]
fn showcase_fixture_has_all_documented_constructs() {
    let nodes = parser::parse(fixture());

    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Heading { level: 1, .. })).is_some(),
        "missing H1"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Heading { level: 2, .. })).is_some(),
        "missing H2"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Heading { level: 3, .. })).is_some(),
        "missing H3"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Strong(_))).is_some(),
        "missing strong"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Emphasis(_))).is_some(),
        "missing emphasis"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Code(_))).is_some(),
        "missing inline code"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Link { .. })).is_some(),
        "missing link"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::CodeBlock { .. })).is_some(),
        "missing code block"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::BlockQuote(_))).is_some(),
        "missing blockquote"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::List { ordered: false, .. })).is_some(),
        "missing ul"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::List { ordered: true, .. })).is_some(),
        "missing ol"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Image { .. })).is_some(),
        "missing image"
    );
    assert!(
        find_node(&nodes, &|n| matches!(n, MdNode::Rule)).is_some(),
        "missing rule"
    );
}

#[test]
fn frontmatter_is_hidden_in_showcase() {
    let nodes = parser::parse(fixture());
    let txt = collect_all_text(&nodes);
    assert!(
        !txt.contains("title: Markdown Showcase"),
        "frontmatter leaked into rendered text: {txt:?}"
    );
}

#[test]
fn registered_markdown_plugin_resolves_for_format_id() {
    let mut registry = PluginRegistry::new();
    registry
        .add_format_plugin(Box::new(MarkdownFormatPlugin::new()))
        .unwrap();
    let plugin = registry
        .format_plugin_for("markdown")
        .expect("plugin registered");
    assert_eq!(plugin.manifest().id, "markdown-note");
}
