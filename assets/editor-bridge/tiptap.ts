// Tiptap backend glue. Lazy-imports Tiptap + StarterKit; the chunk only ships
// when the first .note (richtext-tiptap) tab opens.

import { Editor } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";

import type { BackendInit, EditorState as OperonEditorState, Handle } from "./types.js";

const DEFAULT_DOC = { type: "doc", content: [{ type: "paragraph" }] };

function parseDoc(content: string): unknown {
  if (!content.trim()) return DEFAULT_DOC;
  try {
    return JSON.parse(content);
  } catch {
    return DEFAULT_DOC;
  }
}

export function mountTiptap(target: HTMLElement, init: BackendInit): Handle {
  // Tiptap mounts into a child element, not the host directly — wrap a div
  // so we can clean up cleanly on dispose.
  const inner = document.createElement("div");
  inner.className = "operon-tiptap-inner";
  target.appendChild(inner);

  const initial = parseDoc(init.content);

  const onChangeCallbacks: Array<(content: string) => void> = [];

  const editor = new Editor({
    element: inner,
    extensions: [StarterKit],
    content: initial as any,
    editable: !init.readOnly,
    onUpdate: ({ editor }) => {
      const json = JSON.stringify(editor.getJSON());
      for (const cb of onChangeCallbacks) cb(json);
    },
  });

  let disposed = false;

  const handle: Handle = {
    ready: Promise.resolve(),
    setContent(content: string) {
      if (disposed) return;
      editor.commands.setContent(parseDoc(content) as any);
    },
    getContent(): string {
      if (disposed) return "";
      return JSON.stringify(editor.getJSON());
    },
    onChange(cb) {
      if (disposed) return () => {};
      onChangeCallbacks.push(cb);
      return () => {
        const idx = onChangeCallbacks.indexOf(cb);
        if (idx >= 0) onChangeCallbacks.splice(idx, 1);
      };
    },
    snapshot(): OperonEditorState {
      if (disposed) return { cursor: 0, selection: null, scroll: 0 };
      // ProseMirror offsets are positions in the document; we round-trip
      // them as the cursor field, but they don't translate to Monaco/CM6's
      // string offsets — documented mismatch in src/editor/mod.rs.
      const sel = editor.state.selection;
      return {
        cursor: sel.head,
        selection: sel.empty ? null : [sel.from, sel.to],
        scroll: inner.scrollTop,
      };
    },
    restore(state: OperonEditorState) {
      if (disposed) return;
      try {
        const tr = editor.state.tr;
        // ProseMirror needs Selection objects; for v1 we just set the cursor
        // via a TextSelection at the offset. If the offset is invalid (the
        // document is shorter than the cached cursor), ProseMirror clamps.
        const { TextSelection } = (editor.view as any).state.constructor;
        const sel = TextSelection.create(
          tr.doc,
          Math.min(state.cursor, tr.doc.content.size),
        );
        editor.view.dispatch(tr.setSelection(sel));
      } catch {
        // Safe fallback — out-of-range selection just doesn't restore.
      }
      inner.scrollTop = state.scroll;
    },
    setReadOnly(ro: boolean) {
      if (disposed) return;
      editor.setEditable(!ro);
    },
    setTheme(_theme: unknown) {
      // Tiptap is plain CSS scoped to .operon-tiptap; the surrounding chrome's
      // CSS variables drive its appearance and update automatically on theme
      // signal change. No JS-side reapply needed.
    },
    dispatch(_cmd: string) {
      // Tiptap commands are not exposed via the trait surface in v1.
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      editor.destroy();
      onChangeCallbacks.length = 0;
      try {
        target.removeChild(inner);
      } catch {
        /* host already gone */
      }
    },
  };

  return handle;
}
