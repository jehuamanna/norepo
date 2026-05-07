//! Render an [`MdNode`] tree into Dioxus RSX, themed via Phase-1 token CSS variables.

use dioxus::prelude::*;

use super::nodes::MdNode;
use super::parser;

/// Plans-Phase-5-vfs-wikilinks: optional click resolver for `[[…]]` links.
/// When a Local-Mode shell is mounted it installs this context with a
/// callback that resolves the target text → opens the linked note. Any
/// other surface (cloud Shell, sandboxed preview) leaves the context
/// unset and clicks fall through to the default `href="#"` no-op.
#[derive(Clone, Copy)]
pub struct WikiLinkResolver(pub Callback<String>);

/// Plans-Phase-5-vfs-wikilinks: optional sync checker for "is this wikilink
/// target live?". Returns `true` when the target resolves to a unique note;
/// `false` for not-found or ambiguous. Renderer applies a `wikilink-broken`
/// class when the checker returns `false`.
#[derive(Clone, Copy)]
pub struct WikiLinkChecker(pub Callback<String, bool>);

/// Plans-Phase-6-image-notes (inline-embed): when an `![[Title^short]]`
/// embed wikilink resolves to a `NoteKind::Image` row, the Local-Mode
/// shell installs this resolver to return a `data:<mime>;base64,…` URL
/// for the blob. Renderer uses it to emit `<img src="…">` instead of the
/// text-anchor fallback. `None` means "not an image embed" (falls
/// through to text rendering).
#[derive(Clone, Copy)]
pub struct WikiLinkImageResolver(pub Callback<String, Option<String>>);

/// Standard markdown image (`![alt](src)`) source resolver. The Local-Mode
/// shell installs this so a relative blob path inside a vault (e.g.
/// `.operon/images/<sha>.png` minted by paste-image) resolves to a
/// `data:<mime>;base64,…` URL the wry webview can actually render.
/// External URLs (`http://`, `https://`, `data:`) and unresolved paths
/// pass through unchanged via `None`.
#[derive(Clone, Copy)]
pub struct MarkdownImageResolver(pub Callback<String, Option<String>>);

#[component]
pub fn MarkdownView(content: String) -> Element {
    // Plans-Phase-5-vfs-wikilinks: post-process the AST to lift `[[…]]` and
    // `![[…]]` patterns out of plain Text nodes into typed `WikiLink` nodes.
    let nodes = super::wikilink::expand_wiki(parser::parse(&content));

    // Plans-Phase-9-monaco-desktop (rev 6): empty-state hint so the
    // preview pane in Split / View mode is recognisably "preview, no
    // content yet" rather than "broken / not mounted". Only fires for
    // a body that's both whitespace-only AND parses to zero nodes —
    // anything the parser produces (paragraphs, code blocks, etc.)
    // suppresses the hint immediately.
    let show_empty_hint = nodes.is_empty() && content.trim().is_empty();

    rsx! {
        article {
            class: "markdown-prose",
            "data-testid": "markdown-view",
            if show_empty_hint {
                p {
                    class: "markdown-empty-hint",
                    "data-testid": "markdown-empty-hint",
                    "Type something in the editor to see the preview\u{2026}"
                }
            }
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
        MdNode::Image { dest, alt } => {
            // Try the local-mode resolver first so vault-relative blob
            // paths (`.operon/images/<sha>.png`) inflate to a data: URL
            // the webview can render. Fall back to the literal `dest`
            // for absolute URLs or unresolvable paths.
            let resolved = try_consume_context::<MarkdownImageResolver>()
                .and_then(|MarkdownImageResolver(cb)| cb.call(dest.clone()));
            let src = resolved.unwrap_or_else(|| dest.clone());
            rsx! { img { src: "{src}", alt: "{alt}" } }
        }
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
            // Plans-Phase-5-vfs-wikilinks: when a `WikiLinkResolver` is
            // installed in context (Local-Mode shell), clicking the anchor
            // resolves and routes the target. A `WikiLinkChecker` lets the
            // renderer mark broken targets with a distinct class.
            let resolver = try_consume_context::<WikiLinkResolver>();
            let checker = try_consume_context::<WikiLinkChecker>();
            // Plans-Phase-6-image-notes (inline-embed): if the embed target
            // resolves to an image-note blob, render an `<img>` with the
            // data URL the resolver returns. Falls through to the text
            // anchor when no resolver is installed (e.g. cloud Shell) or
            // the target isn't an image.
            if *embed {
                let img_resolver = try_consume_context::<WikiLinkImageResolver>();
                if let Some(WikiLinkImageResolver(cb)) = img_resolver {
                    if let Some(src) = cb.call(target.clone()) {
                        return rsx! {
                            img {
                                class: "wikilink wikilink-embed wikilink-embed-image",
                                src: "{src}",
                                alt: "{target}",
                                "data-wikilink-target": "{target}",
                                "data-wikilink-embed": "image",
                            }
                        };
                    }
                }
            }
            let live = match checker {
                Some(WikiLinkChecker(cb)) => cb.call(target.clone()),
                None => true,
            };
            // Plans-Phase-9-wikilink-picker (rev 2): when an embed wikilink
            // can't render as `<img>` (no resolver, broken target, or the
            // target turned out to be a markdown note), DON'T show the
            // literal `![[…]]` text — that's indistinguishable from raw
            // source. Drop the leading `!` so the user sees a normal
            // clickable link, and tag the anchor with
            // `data-wikilink-embed="missing"` so themes can still style
            // failed embeds distinctly.
            let display = format!("[[{}]]", target);
            let target_owned = target.clone();
            let onclick = move |evt: Event<MouseData>| {
                if let Some(WikiLinkResolver(cb)) = resolver {
                    evt.prevent_default();
                    cb.call(target_owned.clone());
                }
            };
            let class = if *embed {
                if live {
                    "wikilink wikilink-embed wikilink-embed-missing"
                } else {
                    "wikilink wikilink-embed wikilink-embed-missing wikilink-broken"
                }
            } else if live {
                "wikilink"
            } else {
                "wikilink wikilink-broken"
            };
            let title_attr = if !live {
                format!("Broken link: {}", target)
            } else if *embed {
                format!("Embed target is not an image: {}", target)
            } else {
                target.clone()
            };
            let embed_attr = if *embed { "missing" } else { "false" };
            rsx! {
                a {
                    class,
                    href: "#",
                    "data-wikilink-target": "{target}",
                    "data-wikilink-embed": "{embed_attr}",
                    "data-wikilink-broken": if live { "false" } else { "true" },
                    title: "{title_attr}",
                    onclick,
                    "{display}"
                }
            }
        }
        MdNode::ListItem(_) => rsx! { "" },
        // Plans-Phase-9-monaco-desktop (rev 9): GFM tables. Render a
        // `<table>` with `<thead>`/`<tbody>`. Cell contents are
        // recursively rendered as their inline children. Alignment is
        // intentionally not surfaced yet — pulldown emits per-column
        // alignment but the seed-prompt notes don't rely on it.
        MdNode::Table { headers, rows } => rsx! {
            div {
                class: "markdown-table-wrap",
                table {
                    class: "markdown-table",
                    if !headers.is_empty() {
                        thead {
                            tr {
                                for cell in headers.iter() {
                                    th { {render_children(cell)} }
                                }
                            }
                        }
                    }
                    tbody {
                        for row in rows.iter() {
                            tr {
                                for cell in row.iter() {
                                    td { {render_children(cell)} }
                                }
                            }
                        }
                    }
                }
            }
        },
        MdNode::Strikethrough(c) => rsx! { s { {render_children(c)} } },
        // Internal table-builder nodes never escape the parser, but be
        // exhaustive so a future pulldown-cmark version that surfaces
        // them as top-level events doesn't crash the renderer.
        MdNode::TableRow { .. } | MdNode::TableCell(_) => rsx! { "" },
    }
}

fn render_children(children: &[MdNode]) -> Element {
    rsx! {
        for child in children.iter() {
            {render_node(child)}
        }
    }
}
