// CodeMirror 6 backend glue. Lazy-imports the CM6 modules so the chunk only
// ships when the first LivePreview tab opens.

import { markdown } from "@codemirror/lang-markdown";
import { EditorState, Compartment } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { inlineReplace } from "./cm-inline-replace.js";
import type { BackendInit, EditorState as OperonEditorState, Handle } from "./types.js";

export function mountCodeMirror(target: HTMLElement, init: BackendInit): Handle {
  const themeCompartment = new Compartment();
  const readOnlyCompartment = new Compartment();

  const state = EditorState.create({
    doc: init.content,
    extensions: [
      EditorView.lineWrapping,
      themeCompartment.of(themeExtensionFor(init.theme)),
      readOnlyCompartment.of(EditorState.readOnly.of(init.readOnly)),
      // Markdown grammar — drives the syntaxTree the inline-replace extension walks.
      markdown(),
      inlineReplace,
    ],
  });
  const view = new EditorView({ state, parent: target });

  const onChangeCallbacks: Array<(content: string) => void> = [];
  const docListenerExt = EditorView.updateListener.of((update) => {
    if (!update.docChanged) return;
    const content = update.state.doc.toString();
    for (const cb of onChangeCallbacks) cb(content);
  });
  view.dispatch({ effects: [] });
  // Append the listener as a post-mount reconfigure so onChange registrations
  // happen lazily.
  const listenerCompartment = new Compartment();
  view.dispatch({
    effects: listenerCompartment.reconfigure(docListenerExt),
  });

  let disposed = false;

  const handle: Handle = {
    ready: Promise.resolve(),
    setContent(content: string) {
      if (disposed) return;
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: content },
      });
    },
    getContent(): string {
      if (disposed) return "";
      return view.state.doc.toString();
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
      const head = view.state.selection.main.head;
      const sel = view.state.selection.main;
      const selection: [number, number] | null =
        sel.from !== sel.to ? [sel.from, sel.to] : null;
      return { cursor: head, selection, scroll: view.scrollDOM.scrollTop };
    },
    restore(state: OperonEditorState) {
      if (disposed) return;
      try {
        view.dispatch({
          selection: { anchor: state.cursor, head: state.cursor },
        });
      } catch {
        // Out-of-range cursor — clamp silently.
      }
      view.scrollDOM.scrollTop = state.scroll;
    },
    setReadOnly(ro: boolean) {
      if (disposed) return;
      view.dispatch({
        effects: readOnlyCompartment.reconfigure(EditorState.readOnly.of(ro)),
      });
    },
    setTheme(theme: unknown) {
      if (disposed) return;
      view.dispatch({
        effects: themeCompartment.reconfigure(themeExtensionFor(theme)),
      });
    },
    dispatch(_cmd: string) {
      // CM6 has its own command system; we don't expose every command for v1.
      // Future commits can route Undo/Redo through view.dispatch / runCommand.
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      view.destroy();
      onChangeCallbacks.length = 0;
    },
  };

  return handle;
}

function themeExtensionFor(theme: unknown): readonly any[] {
  // Honest minimum for v1: a tiny theme that makes the editor readable on
  // dark + light. The Operon CSS variables drive the surrounding chrome; CM6
  // styles its own canvas. A richer per-palette theme is a follow-up.
  if (typeof theme === "string" && (theme === "vs-dark" || theme === "hc-black")) {
    return [
      EditorView.theme({
        "&": { color: "#d4d4d4", backgroundColor: "transparent" },
        ".cm-content": { caretColor: "#aeafad" },
        ".cm-cursor": { borderLeftColor: "#aeafad" },
      }),
    ];
  }
  return [
    EditorView.theme({
      "&": { color: "#1e1e1e", backgroundColor: "transparent" },
      ".cm-content": { caretColor: "#1e1e1e" },
      ".cm-cursor": { borderLeftColor: "#1e1e1e" },
    }),
  ];
}
