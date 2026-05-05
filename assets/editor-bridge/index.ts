// Operon editor bridge — entry point.
//
// Exposes a single `mount(target, init)` function that the Rust EditorBackend
// impls call via wasm-bindgen. The `init.kind` selects the underlying library
// and triggers a dynamic import so chunks ship lazily (Monaco only loads when
// the first edit-capable tab opens; CM6 only when Live Preview engages; Tiptap
// only when a .note file opens).

import type { BackendInit, Handle } from "./types.js";

declare global {
  interface Window {
    /** Loaded-libraries marker used by tests to assert lazy-load behaviour. */
    __operon_loaded?: Set<string>;
  }
}

function markLoaded(kind: string) {
  if (typeof window !== "undefined") {
    (window.__operon_loaded ??= new Set()).add(kind);
  }
}

export async function mount(target: HTMLElement, init: BackendInit): Promise<Handle> {
  switch (init.kind) {
    case "monaco": {
      const { mountMonaco } = await import("./monaco.js");
      markLoaded("monaco");
      return mountMonaco(target, init);
    }
    case "codemirror": {
      // Phase 4 lands the CM6 backend.
      throw new Error("codemirror backend not yet implemented");
    }
    case "tiptap": {
      // Phase 5 lands the Tiptap backend.
      throw new Error("tiptap backend not yet implemented");
    }
    default: {
      const _exhaustive: never = init.kind;
      throw new Error(`unknown backend kind: ${String(_exhaustive)}`);
    }
  }
}

// Re-export shared types so consumers can import { Handle } from this entry.
export type { Handle, BackendInit, EditorState, BackendKind } from "./types.js";
