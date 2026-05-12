// Monaco-specific glue. Mounts a Monaco editor in `target`, returns a Handle
// matching the bridge contract. The `ready` promise resolves once Monaco's
// constructor returns (Monaco mounts synchronously after the dynamic import
// completes, but we keep `ready` as a Promise to match the trait shape and to
// allow future async setup like worker initialization).

import * as monaco from "monaco-editor";

import type { BackendInit, EditorState, Handle } from "./types.js";

// Plans-Phase-9-monaco-desktop (rev 5): Monaco's ESM build imports its
// own CSS via plain `import './foo.css'` statements. Our esbuild build
// uses `--loader:.css=text` which returns the CSS as a string but
// doesn't auto-inject it — so the editor mounts but renders without
// styles (no gutter, no proper scrollbars, content positioned wrong).
// Force-import the pre-bundled `min/vs/editor/editor.main.css` (one
// file, ~128KB, covers every Monaco feature) and inject once before
// the first `editor.create()` call. Idempotent so second-and-later
// editor instances skip re-injection.
//
// `@ts-expect-error` — esbuild text loader returns a string but
// TypeScript doesn't know that without a declaration.
// @ts-expect-error
import editorMainCss from "monaco-editor/min/vs/editor/editor.main.css";

let monacoCssInjected = false;
function ensureMonacoCss(): void {
  if (monacoCssInjected) return;
  if (typeof document === "undefined") return;
  const style = document.createElement("style");
  style.id = "operon-monaco-injected-css";
  style.setAttribute("data-source", "monaco-editor/min/vs/editor/editor.main.css");
  style.textContent = editorMainCss as string;
  document.head.appendChild(style);
  monacoCssInjected = true;
}

export function mountMonaco(target: HTMLElement, init: BackendInit): Handle {
  ensureMonacoCss();
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
    replaceRange(start: number, end: number, text: string) {
      if (disposed) return;
      const model = editor.getModel();
      if (!model) return;
      const total = model.getValueLength();
      const s = Math.max(0, Math.min(start, total));
      const e = Math.max(s, Math.min(end, total));
      const startPos = model.getPositionAt(s);
      const endPos = model.getPositionAt(e);
      const range = new monaco.Range(
        startPos.lineNumber,
        startPos.column,
        endPos.lineNumber,
        endPos.column,
      );
      // executeEdits routes through the editor's command stack so the
      // edit lands in the undo history; setContent / model.setValue
      // would replace the entire model and reset that history,
      // which is why paste / cut / splice must use this entry point.
      editor.executeEdits("operon-bridge", [
        { range, text, forceMoveMarkers: true },
      ]);
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
    layout() {
      if (disposed) return;
      // Plans-Phase-9-monaco-desktop (rev 7): re-measure the container
      // and reflow Monaco's DOM. Useful when the host element gets its
      // final flex size after `monaco.editor.create()` already ran with
      // a 0x0 measurement.
      editor.layout();
    },
    dispatch(cmd: string) {
      if (disposed) return;
      // RevealLine:N — reveal a 1-indexed line in center, place caret at col 1,
      // and steal focus so keyboard scrolling/selection picks up immediately.
      if (cmd.startsWith("RevealLine:")) {
        const raw = cmd.slice("RevealLine:".length);
        const lineNumber = Math.max(1, parseInt(raw, 10) || 1);
        try {
          editor.revealLineInCenter(lineNumber);
          editor.setPosition({ lineNumber, column: 1 });
          editor.focus();
        } catch (e) {
          console.warn("[operon-bridge] revealLine failed", e);
        }
        return;
      }
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
        case "Focus":
          // Plans-Phase-2-editor-auto-focus: route to Monaco's native focus
          // call so the keyboard caret moves into the editor body without
          // requiring an additional user click.
          editor.focus();
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
