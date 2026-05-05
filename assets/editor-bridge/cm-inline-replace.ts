// Obsidian-style inline-replace decoration extension for CodeMirror 6.
//
// Walks the markdown syntax tree on every viewport / selection update and
// replaces certain token ranges with rendered widgets:
//
//   **bold**      → <strong>bold</strong>
//   *italic*      → <em>italic</em>
//   `code`        → <code>code</code>
//   # Heading     → just "Heading" with a font-size class on the line
//   [link](url)   → just "link" styled as an anchor (no nav)
//
// When the cursor enters a replaced range, that range's decoration is
// suppressed so the source asterisks/underscores reappear and the user can
// edit them.

import { syntaxTree } from "@codemirror/language";
import {
  Decoration,
  DecorationSet,
  EditorView,
  ViewPlugin,
  ViewUpdate,
  WidgetType,
} from "@codemirror/view";

class InlineWidget extends WidgetType {
  constructor(private readonly text: string, private readonly tag: string) {
    super();
  }
  toDOM(): HTMLElement {
    const el = document.createElement(this.tag);
    el.className = "operon-cm-inline-replace";
    el.textContent = this.text;
    return el;
  }
  override eq(other: WidgetType): boolean {
    return (
      other instanceof InlineWidget &&
      other.text === this.text &&
      other.tag === this.tag
    );
  }
}

function cursorOnRange(view: EditorView, from: number, to: number): boolean {
  for (const range of view.state.selection.ranges) {
    if (range.from <= to && range.to >= from) return true;
  }
  return false;
}

function buildDecorations(view: EditorView): DecorationSet {
  const builder: Array<{ from: number; to: number; deco: Decoration }> = [];
  const tree = syntaxTree(view.state);
  for (const { from, to } of view.visibleRanges) {
    tree.iterate({
      from,
      to,
      enter: (node) => {
        if (cursorOnRange(view, node.from, node.to)) {
          // Cursor in this token — let the source markers show.
          return;
        }
        const inner = view.state.doc.sliceString(node.from, node.to);
        switch (node.name) {
          case "StrongEmphasis": {
            // **text** or __text__ — strip the four delimiter chars
            const text = inner.replace(/^(\*\*|__)|(\*\*|__)$/g, "");
            builder.push({
              from: node.from,
              to: node.to,
              deco: Decoration.replace({ widget: new InlineWidget(text, "strong") }),
            });
            break;
          }
          case "Emphasis": {
            const text = inner.replace(/^[*_]|[*_]$/g, "");
            builder.push({
              from: node.from,
              to: node.to,
              deco: Decoration.replace({ widget: new InlineWidget(text, "em") }),
            });
            break;
          }
          case "InlineCode": {
            const text = inner.replace(/^`|`$/g, "");
            builder.push({
              from: node.from,
              to: node.to,
              deco: Decoration.replace({ widget: new InlineWidget(text, "code") }),
            });
            break;
          }
          // Heading marks: hide the leading `# `, leave the heading text visible. We
          // handle this by replacing only the HeaderMark child node.
          case "HeaderMark": {
            builder.push({
              from: node.from,
              to: Math.min(node.to + 1, view.state.doc.length), // include the trailing space
              deco: Decoration.replace({ widget: new InlineWidget("", "span") }),
            });
            break;
          }
          // Link: [text](url) — replace the whole node with just the visible text.
          case "Link": {
            const m = /^\[([^\]]*)\]\(([^)]*)\)/.exec(inner);
            if (m) {
              builder.push({
                from: node.from,
                to: node.to,
                deco: Decoration.replace({
                  widget: new InlineWidget(m[1] ?? "", "a"),
                }),
              });
            }
            break;
          }
          default:
            return;
        }
      },
    });
  }
  // Decoration.set requires sorted ranges with no overlaps.
  builder.sort((a, b) => a.from - b.from || a.to - b.to);
  const dedup: typeof builder = [];
  for (const r of builder) {
    const last = dedup[dedup.length - 1];
    if (last && r.from < last.to) continue;
    dedup.push(r);
  }
  return Decoration.set(dedup.map((r) => r.deco.range(r.from, r.to)));
}

export const inlineReplace = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }
    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged
      ) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);
