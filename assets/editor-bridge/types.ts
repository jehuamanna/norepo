// Shared interfaces for the Operon editor bridge. The Rust EditorBackend impls
// hold a JsValue handle whose shape matches `Handle` below.

export type BackendKind = "monaco" | "codemirror" | "tiptap";

export interface EditorState {
  cursor: number;
  selection: [number, number] | null;
  scroll: number;
}

export interface BackendInit {
  kind: BackendKind;
  languageId: string;
  content: string;
  theme: unknown;
  readOnly: boolean;
}

export interface Handle {
  /** Resolves once the underlying editor library has finished mounting. Tests
   * gate on this; no setTimeout retry-loops. */
  ready: Promise<void>;
  setContent(content: string): void;
  getContent(): string;
  /** Replace the model text in `[start, end)` with `text`, pushing the
   * edit through the editor's executeEdits API so it joins the
   * native undo/redo stack. Used by paste/cut/splice paths that need
   * Ctrl+Z to round-trip cleanly — `setContent` clobbers history, so
   * those paths must not use it. Offsets are 0-based UTF-16 code-unit
   * positions in the model (same units `snapshot.cursor` /
   * `snapshot.selection` use). */
  replaceRange(start: number, end: number, text: string): void;
  /** Subscribe to content changes. Returns an unsubscribe fn that disposes the
   * registered closure on the JS side. */
  onChange(cb: (content: string) => void): () => void;
  snapshot(): EditorState;
  restore(state: EditorState): void;
  setReadOnly(ro: boolean): void;
  setTheme(theme: unknown): void;
  dispatch(cmd: string): void;
  /** Force the underlying editor to re-measure its container and lay out
   * its DOM. Needed when the editor mounts inside a flex-nested host
   * whose final size is resolved AFTER `editor.create(...)` returns —
   * Monaco's automaticLayout ResizeObserver doesn't always fire in that
   * sequence. Plans-Phase-9-monaco-desktop (rev 7). */
  layout(): void;
  /** Tear down the editor and free every closure that crossed the wasm-bindgen
   * boundary. Once called, the handle is unusable. */
  dispose(): void;
}
