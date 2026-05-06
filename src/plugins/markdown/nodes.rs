//! Typed AST emitted by [`super::parser::parse`].
//!
//! [`MdNode::ListItem`] is an internal intermediate node used while building lists from
//! pulldown-cmark events; it is unpacked into [`MdNode::List`]'s `items` and never appears
//! in the public output of `parse`.

#[derive(Clone, Debug, PartialEq)]
pub enum MdNode {
    Heading { level: u8, children: Vec<MdNode> },
    Paragraph { children: Vec<MdNode> },
    Text(String),
    Strong(Vec<MdNode>),
    Emphasis(Vec<MdNode>),
    Link { dest: String, title: String, children: Vec<MdNode> },
    Image { dest: String, alt: String },
    Code(String),
    CodeBlock { lang: Option<String>, code: String },
    BlockQuote(Vec<MdNode>),
    List { ordered: bool, items: Vec<Vec<MdNode>> },
    Rule,
    /// Plans-Phase-5-vfs-wikilinks: Obsidian-style `[[Project/Note]]` link.
    /// `embed=true` for `![[…]]` (image-note embeds, Plans-Phase-6).
    /// `target` carries the raw inner text (e.g. `"Project/Note"` or
    /// `"Note^abc12345"`); resolution happens at render time once the
    /// `vfs::resolve_link` infrastructure lands.
    WikiLink { target: String, embed: bool },
    /// Internal: closed `<li>`-style item — collected when building a [`MdNode::List`].
    ListItem(Vec<MdNode>),
}
