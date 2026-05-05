// Monaco-specific glue. Mounts a Monaco editor in `target`, returns a Handle
// matching the bridge contract. The `ready` promise resolves once Monaco's
// constructor returns (Monaco mounts synchronously after the dynamic import
// completes, but we keep `ready` as a Promise to match the trait shape and to
// allow future async setup like worker initialization).

import * as monaco from "monaco-editor";

import type { BackendInit, EditorState, Handle } from "./types.js";

export function mountMonaco(target: HTMLElement, init: BackendInit): Handle {
  const editor = monaco.editor.create(target, {
    value: init.content,
    language: init.languageId,
    automaticLayout: true,
    readOnly: init.readOnly,
    theme: typeof init.theme === "string" ? init.theme : "vs",
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    fontSize: 14,
  });

  // Closures crossing the wasm-bindgen boundary live here so `dispose` can free
  // them deterministically. Without this, every `onChange` registration leaks
  // an editor instance.
  const disposers: Array<() => void> = [];
  let disposed = false;

  const ready: Promise<void> = Promise.resolve();

  const handle: Handle = {
    ready,
    setContent(content: string) {
      if (disposed) return;
      const model = editor.getModel();
      if (model) model.setValue(content);
    },
    getContent(): string {
      if (disposed) return "";
      return editor.getValue();
    },
    onChange(cb) {
      if (disposed) return () => {};
      const sub = editor.onDidChangeModelContent(() => cb(editor.getValue()));
      const unsub = () => sub.dispose();
      disposers.push(unsub);
      return unsub;
    },
    snapshot(): EditorState {
      if (disposed) return { cursor: 0, selection: null, scroll: 0 };
      const pos = editor.getPosition();
      const sel = editor.getSelection();
      const cursorOffset = pos
        ? editor.getModel()?.getOffsetAt(pos) ?? 0
        : 0;
      const selectionRange: [number, number] | null = sel && !sel.isEmpty()
        ? [
            editor.getModel()?.getOffsetAt(sel.getStartPosition()) ?? 0,
            editor.getModel()?.getOffsetAt(sel.getEndPosition()) ?? 0,
          ]
        : null;
      const scroll = editor.getScrollTop();
      return { cursor: cursorOffset, selection: selectionRange, scroll };
    },
    restore(state: EditorState) {
      if (disposed) return;
      const model = editor.getModel();
      if (!model) return;
      const pos = model.getPositionAt(state.cursor);
      editor.setPosition(pos);
      if (state.selection) {
        const start = model.getPositionAt(state.selection[0]);
        const end = model.getPositionAt(state.selection[1]);
        editor.setSelection({
          startLineNumber: start.lineNumber,
          startColumn: start.column,
          endLineNumber: end.lineNumber,
          endColumn: end.column,
        });
      }
      editor.setScrollTop(state.scroll);
    },
    setReadOnly(ro: boolean) {
      if (disposed) return;
      editor.updateOptions({ readOnly: ro });
    },
    setTheme(theme: unknown) {
      if (disposed) return;
      if (typeof theme === "string") {
        monaco.editor.setTheme(theme);
      } else if (theme && typeof theme === "object" && "name" in theme) {
        const t = theme as { name: string; data?: monaco.editor.IStandaloneThemeData };
        if (t.data) monaco.editor.defineTheme(t.name, t.data);
        monaco.editor.setTheme(t.name);
      }
    },
    dispatch(cmd: string) {
      if (disposed) return;
      switch (cmd) {
        case "Undo":
          editor.trigger("bridge", "undo", null);
          break;
        case "Redo":
          editor.trigger("bridge", "redo", null);
          break;
        case "FormatDocument":
          editor.trigger("bridge", "editor.action.formatDocument", null);
          break;
        case "FindReplace":
          editor.trigger("bridge", "editor.action.startFindReplaceAction", null);
          break;
        case "ToggleComment":
          editor.trigger("bridge", "editor.action.commentLine", null);
          break;
        default:
          // Unknown command — log to ease debugging without throwing across the
          // wasm-bindgen boundary.
          console.warn(`[operon-bridge] unknown editor command: ${cmd}`);
      }
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      while (disposers.length) {
        const d = disposers.pop();
        try { d?.(); } catch (e) { console.warn("[operon-bridge] disposer threw", e); }
      }
      editor.dispose();
    },
  };

  return handle;
}
