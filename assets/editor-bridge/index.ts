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
    /** Bridge surface the Rust EditorBackend impls call into via wasm-bindgen.
     * Populated by this entry script on first import. */
    operonBridge?: {
      mount: (target: HTMLElement, init: BackendInit) => Promise<Handle>;
    };
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
      const { mountCodeMirror } = await import("./codemirror.js");
      markLoaded("codemirror");
      return mountCodeMirror(target, init);
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

// Install the bridge on the window global so wasm-bindgen-side code can call it without a
// static module reference. Idempotent — re-importing this module won't shadow an existing
// installation.
if (typeof window !== "undefined" && !window.operonBridge) {
  window.operonBridge = { mount };
}

// Re-export shared types so consumers can import { Handle } from this entry.
export type { Handle, BackendInit, EditorState, BackendKind } from "./types.js";
