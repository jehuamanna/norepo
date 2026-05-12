//! Shared Monaco-mounting component used by every source-text format plugin's `render_edit`.
//!
//! Owns a `MonacoBackend` for the lifetime of its mounted DOM node: spawns the async mount
//! once the target `<div>` is in the tree, registers `on_change` so the plugin's
//! `EventHandler<String>` fires on every content mutation, and disposes the backend when the
//! component unmounts (Dioxus's `use_drop` runs the cleanup).
//!
//! A single component shared across markdown / plaintext / json / mdx avoids four copies of
//! the wasm-bindgen wiring — Plans-Phase-0's "shared bridge layer" rationale.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

/// Plans-Phase-9-monaco-desktop (rev 1): handle parents (e.g.
/// `LocalNoteEditor`) hold to push programmatic content updates into a
/// mounted Monaco instance — wikilink picker insert, image-paste splice,
/// drop-image splice, image-picker splice, etc. The wrapped `Eval` is the
/// same long-lived script the host owns; `set_content` enqueues a
/// `setContent` message into the JS-side `dioxus.recv()` loop.
///
/// Desktop-only because the wasm path uses `MonacoBackend` directly via
/// wasm-bindgen rather than `document::eval`. `Eval` itself is available on
/// both targets, so the type compiles everywhere — but `set_content` is a
/// no-op until the host's bootstrap script enters its message loop.
#[derive(Clone)]
pub struct MonacoChannel {
    eval: document::Eval,
    /// Per-mount host id, also keyed in `window.__operon_monaco_handles`.
    /// Lets `splice` issue a fresh `document::eval` that grabs the live
    /// Monaco handle directly, bypassing the unreliable bootstrap recv
    /// queue (see `splice` for the full rationale).
    host_id: String,
}

impl MonacoChannel {
    /// Replace Monaco's buffer with `value`. Used after wikilink-picker /
    /// paste-image / drop-image splices so Monaco's view stays in sync
    /// with the `Tab.content` mirror Rust holds. The JS side suppresses
    /// the resulting `change` event so the user-input round-trip can't
    /// loop.
    pub fn set_content(&self, value: &str) {
        let _ = self
            .eval
            .send(serde_json::json!({"type":"setContent","value":value}));
    }

    /// Move the keyboard caret into Monaco. Mirrors the wasm path's
    /// `EditorCommand::Focus`.
    pub fn focus(&self) {
        let _ = self.eval.send(serde_json::json!({"type":"focus"}));
    }

    /// Reveal a 1-indexed line, place the caret at column 1, and focus.
    /// Mirrors the wasm path's `EditorCommand::RevealLine(n)`.
    pub fn reveal_line(&self, line_number: u32) {
        let _ = self
            .eval
            .send(serde_json::json!({"type":"revealLine","value":line_number}));
    }

    /// Insert `text` at Monaco's current caret position.
    ///
    /// Earlier revs sent `{type:"splice"}` through the bootstrap's
    /// `await dioxus.recv()` loop (the same long-lived `Eval` that
    /// owns the mount). That path turned out to be unreliable for
    /// post-mount sends — same hazard the prop-mirror `setContent`
    /// path documents at the bottom of `MonacoEditorHost`: the recv
    /// queue silently drops messages, leaving the Monaco buffer
    /// untouched. The user-visible symptom was "link-picker fires,
    /// `eval.send` returns Ok, nothing happens in Monaco."
    ///
    /// Fix: bypass the recv queue and issue a fresh `document::eval`
    /// that grabs `window.__operon_monaco_handles[hostId]` and runs
    /// the `snapshot/getContent/slice/setContent/restore` sequence
    /// inline. We don't set the suppress flag, so Monaco's onChange
    /// (registered in the bootstrap) routes the new content back to
    /// Rust through the bootstrap's safeSend → `Tab.content` mirror
    /// updates as before. We do NOT also call `eval.send` here — the
    /// recv-path splice would *also* insert at the cursor, doubling
    /// the text on the rare occasion both paths land.
    pub fn splice(&self, text: &str) {
        eprintln!(
            "operon: MonacoChannel::splice direct-eval len={} preview={:?}",
            text.len(),
            &text.chars().take(40).collect::<String>()
        );
        let host_id_json = serde_json::to_string(&self.host_id)
            .unwrap_or_else(|_| String::from("\"\""));
        let text_json = serde_json::to_string(text)
            .unwrap_or_else(|_| String::from("\"\""));
        let direct_script = format!(
            "(function() {{ \
                const hid = {host_id_json}; \
                const h = (window.__operon_monaco_handles || {{}})[hid]; \
                if (!h) {{ console.warn('operon: direct splice: no handle for', hid); return; }} \
                try {{ \
                    const state = h.snapshot(); \
                    const old = h.getContent(); \
                    const text = {text_json}; \
                    let s, e; \
                    if (state && state.selection) {{ \
                        s = state.selection[0]; e = state.selection[1]; \
                    }} else {{ \
                        const pos = Math.min((state && state.cursor) || 0, old.length); \
                        s = pos; e = pos; \
                    }} \
                    if (typeof h.replaceRange === 'function') {{ \
                        h.replaceRange(s, e, text); \
                    }} else {{ \
                        /* legacy bridge fallback — non-undoable */ \
                        const next = old.slice(0, s) + text + old.slice(e); \
                        h.setContent(next); \
                        try {{ h.restore({{cursor: s + text.length, selection: null, scroll: (state && state.scroll) || 0}}); }} catch (e2) {{}} \
                    }} \
                    try {{ if (typeof h.layout === 'function') h.layout(); }} catch (e) {{}} \
                    console.log('operon: direct splice OK', hid, 'textLen', text.length); \
                }} catch (e) {{ \
                    console.warn('operon: direct splice threw', hid, e); \
                }} \
            }})();",
        );
        let _ = document::eval(&direct_script);
    }
}

#[component]
pub fn MonacoEditorHost(
    note_id: String,
    content: String,
    language: LanguageDescriptor,
    on_change: EventHandler<String>,
    /// Plans-Phase-9-monaco-desktop (rev 1): when present, the host
    /// writes a `MonacoChannel` here once the eval handle exists, so
    /// the parent can push `setContent` from the picker / paste / drop
    /// callsites. Optional so existing callers (cloud render path) need
    /// no change.
    #[props(default)]
    channel_sink: Option<Signal<Option<MonacoChannel>>>,
    /// Plans-Phase-9-monaco-desktop (rev 1): keyboard bindings that
    /// Monaco normally swallows (Cmd+S, Cmd+K, Cmd+Shift+I, Cmd+Z, Cmd+Shift+Z).
    /// The bootstrap script intercepts them at the window with
    /// `capture: true`, calls `preventDefault`, and posts the action
    /// name. The wasm path doesn't use this — it has its own dispatch
    /// system. Optional so cloud callers can ignore.
    #[props(default)]
    on_action: Option<EventHandler<String>>,
    /// Mirror of the host's private `monaco_ready: Signal<bool>` for the
    /// parent. Flips true when the JS bootstrap finishes mounting Monaco
    /// and emits `{type:"mounted"}`. The paste / splice handlers in
    /// `LocalNoteEditor` gate their `MonacoChannel.splice(...)` calls on
    /// this so the message isn't sent into the void before the JS recv
    /// loop has started — see the long comment on the existing
    /// `setContent` gate (rev 19) for why pre-handshake messages are
    /// silently dropped.
    #[props(default)]
    ready_sink: Option<Signal<bool>>,
) -> Element {
    // The `data-*` attributes let Playwright + wasm-bindgen tests assert the mount fired.
    // Capability-honest: this surface only renders when the active plugin claims `EDIT`,
    // so reaching here means a Monaco mount is desired.
    let note_id_attr = note_id.clone();
    let language_attr = language.id;

    // Plans-Phase-9-monaco-desktop (rev 1): Dioxus desktop runs Rust
    // natively, but the webview hosting our DOM can run JavaScript via
    // `document::eval`. We spawn ONE long-lived eval per host instance
    // that dynamic-imports the existing editor-bridge JS (the same one
    // the wasm path uses), mounts Monaco against the host `<div>`, and
    // enters a bidirectional message loop:
    //   - Monaco's `onChange` posts back via `dioxus.send({type:"change"})`.
    //   - `eval.send({type:"setContent" | "focus" | "dispose"})` pushes
    //     commands the other way (used by the picker / paste / drop /
    //     unmount paths).
    // The wasm path is unchanged.
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Plans-Phase-9-monaco-desktop (rev 13): unique host id per
        // mount instance. Earlier revs used `operon-monaco-host-
        // {note_id}` which made every Local-Mode editor for the same
        // note share an id. During an Edit↔Split mode transition,
        // Dioxus briefly keeps both panes' DOM in the document while
        // diffing — `document.getElementById` then returned the OLD
        // pane's host, the bootstrap mounted Monaco against it, and
        // a tick later Dioxus removed that OLD pane (and the editor
        // with it) leaving the NEW pane's host empty. View↔Edit /
        // View↔Split worked because View mode doesn't render
        // `LocalNoteEditor` so there's no second host to confuse
        // the lookup. Unique counter via `use_hook` survives
        // re-renders and never collides across remounts.
        use std::sync::atomic::{AtomicU64, Ordering};
        let host_seq: u64 = use_hook(|| {
            static MONACO_HOST_SEQ: AtomicU64 = AtomicU64::new(0);
            MONACO_HOST_SEQ.fetch_add(1, Ordering::Relaxed)
        });
        let host_id = format!("operon-monaco-host-{note_id}-{host_seq}");
        let initial_content = content.clone();
        let language_id = language.id.to_string();

        // Build the bootstrap script once, capturing the per-host id +
        // initial content. Idempotent: re-renders don't re-run because
        // we stash the resulting Eval in `use_hook`.
        let host_id_for_script = host_id.clone();
        let initial_content_json = serde_json::to_string(&initial_content)
            .unwrap_or_else(|_| String::from("\"\""));
        let language_id_json = serde_json::to_string(&language_id)
            .unwrap_or_else(|_| String::from("\"plaintext\""));

        let eval_handle: document::Eval = use_hook(move || {
            let host_id = host_id_for_script;
            let script = format!(
                r#"(async function() {{
                    try {{
                        if (!window.operonBridge) {{
                            // Plans-Phase-9-monaco-desktop (rev 2): the
                            // bridge dist is served via the custom
                            // `bridge://` Wry protocol registered in
                            // `main::bridge_protocol_handler`. Wry
                            // doesn't auto-serve `/assets/` for desktop
                            // the way `dx serve --target web` does, so
                            // the wasm-style URL would 404 here.
                            try {{
                                await import('bridge://localhost/index.js');
                            }} catch (impErr) {{
                                try {{ dioxus.send({{type:"error", message:"import failed: "+String(impErr && impErr.message || impErr)}}); }} catch (_) {{}}
                                return;
                            }}
                        }}
                        if (!window.operonBridge) {{
                            try {{ dioxus.send({{type:"error", message:"bridge not loaded"}}); }} catch (_) {{}}
                            return;
                        }}
                        // Plans-Phase-9-monaco-desktop (rev 14): the
                        // unique-host-id fix landed in rev 13 means we
                        // never collide with a sibling pane's host
                        // element, so the size-gated polling and
                        // multi-stage relayout retries from earlier
                        // revs are no longer needed. Wait briefly for
                        // the element to flush into the DOM, then
                        // mount.
                        let target = document.getElementById('{host_id}');
                        let attempts = 0;
                        while (!target && attempts < 60) {{
                            await new Promise(r => setTimeout(r, 33));
                            target = document.getElementById('{host_id}');
                            attempts++;
                        }}
                        if (!target) {{
                            try {{ dioxus.send({{type:"error", message:"host element not found"}}); }} catch (_) {{}}
                            return;
                        }}
                        const handle = await window.operonBridge.mount(target, {{
                            kind: "monaco",
                            languageId: {language_id_json},
                            content: {initial_content_json},
                            theme: "vs",
                            readOnly: false,
                        }});
                        window.__operon_monaco_handles = window.__operon_monaco_handles || {{}};
                        window.__operon_monaco_handles['{host_id}'] = handle;
                        // Suppress change events fired by programmatic
                        // setContent so Rust doesn't see its own write
                        // bounce back as user input.
                        let suppress = false;
                        // Wrap dioxus.send in try/catch — Monaco keeps
                        // firing onChange events on this handle even
                        // after the Rust-side `Eval` has been dropped
                        // (e.g. tab unmount, hot reload). The stale
                        // dioxus.send then throws `null is not an object
                        // (window.getQuery(N).rustSend)` and floods the
                        // console. Same protection for the keydown +
                        // mounted senders below.
                        const safeSend = (msg) => {{
                            try {{ dioxus.send(msg); }} catch (e) {{ /* eval dropped */ }}
                        }};
                        handle.onChange((c) => {{
                            if (suppress) return;
                            // Respect the cross-eval suppress flag so the
                            // direct setContent path can also silence
                            // bounce-back changes.
                            if (window.__operon_suppress_change && window.__operon_suppress_change['{host_id}']) return;
                            safeSend({{type:"change", value:c}});
                        }});
                        // Plans-Phase-9-monaco-desktop: capture-phase
                        // keybindings Monaco normally swallows (Cmd+S,
                        // Cmd+K, Cmd+Shift+I). PreventDefault stops
                        // browser save/page-source; the action name
                        // routes back to Rust so LocalNoteEditor can
                        // wire it to the existing handlers.
                        const onKey = (ev) => {{
                            if (!target.contains(ev.target)) return;
                            const meta = ev.metaKey || ev.ctrlKey;
                            if (meta) {{
                                const key = ev.key && ev.key.toLowerCase();
                                // Cmd/Ctrl+V: Linux WebKitGTK silently
                                // drops text from `clipboardData.getData`,
                                // so Monaco's native paste produces
                                // nothing. Bypass by routing through
                                // Rust's arboard reader. We `preventDefault`
                                // (kills native paste) but do NOT
                                // `stopPropagation` so the document-level
                                // image-paste handler in LocalNoteEditor
                                // (and the artifact view) still fires
                                // afterwards and can claim image bytes.
                                if (key === "v" && !ev.shiftKey && !ev.altKey) {{
                                    try {{ console.log("operon: monaco capture Ctrl+V", '{host_id}', "target_contains_active=", target.contains(document.activeElement)); }} catch (e) {{}}
                                    ev.preventDefault();
                                    // Block Monaco's bubble-phase Ctrl+V
                                    // keybinding from also firing — its
                                    // native paste action reads an empty
                                    // clipboardData on wry/WebKitGTK and
                                    // silently deletes the selection
                                    // (looks like an undo to the user).
                                    // Document-level image-paste rides on
                                    // the `paste` event, not keydown, so
                                    // it is not affected.
                                    ev.stopImmediatePropagation();
                                    safeSend({{type:"paste"}});
                                    return;
                                }}
                                // Cmd/Ctrl+Z (undo) and Cmd+Shift+Z /
                                // Cmd+Y (redo): Monaco's native undo
                                // keybindings ride on browser commands
                                // that WebKitGTK doesn't always wire
                                // through to focused contenteditable
                                // textareas in wry. Bypass by triggering
                                // the bridge's undo/redo command directly
                                // — same path the bridge's `dispatch`
                                // entry point uses.
                                if (key === "z" && !ev.altKey) {{
                                    ev.preventDefault();
                                    try {{
                                        handle.dispatch(ev.shiftKey ? "Redo" : "Undo");
                                    }} catch (e) {{ console.warn("operon: undo/redo dispatch threw", e); }}
                                    return;
                                }}
                                if (key === "y" && !ev.shiftKey && !ev.altKey) {{
                                    ev.preventDefault();
                                    try {{ handle.dispatch("Redo"); }}
                                    catch (e) {{ console.warn("operon: redo dispatch threw", e); }}
                                    return;
                                }}
                                // Cmd/Ctrl+C (copy) and Cmd/Ctrl+X (cut):
                                // same WebKitGTK clipboardData blackhole
                                // breaks Monaco's native copy/cut. We
                                // derive the selected text JS-side via
                                // `handle.snapshot()` + `handle.getContent()`,
                                // ship it to Rust which writes it to the
                                // OS clipboard via arboard. For cut, we
                                // also splice the selection out locally
                                // so Monaco's view + undo stack stay in
                                // sync. When the cursor has no selection,
                                // we operate on the *current line* (cut)
                                // or copy the line text — same convention
                                // VSCode / JetBrains use.
                                if ((key === "c" || key === "x") && !ev.shiftKey && !ev.altKey) {{
                                    let snap, content;
                                    try {{
                                        snap = handle.snapshot();
                                        content = handle.getContent();
                                    }} catch (e) {{ return; }}
                                    let selStart = 0;
                                    let selEnd = 0;
                                    if (snap && snap.selection) {{
                                        selStart = snap.selection[0];
                                        selEnd = snap.selection[1];
                                    }}
                                    const noSelection = selStart === selEnd;
                                    let cutText;
                                    let removeStart;
                                    let removeEnd;
                                    if (noSelection) {{
                                        // Current-line behavior. Find
                                        // line bounds around the caret;
                                        // include the trailing newline
                                        // so cutting the line collapses
                                        // cleanly without leaving an
                                        // empty hanger.
                                        const cur = (snap && snap.cursor) || 0;
                                        let lineStart = cur;
                                        while (lineStart > 0 && content[lineStart - 1] !== "\n") lineStart--;
                                        let lineEnd = cur;
                                        while (lineEnd < content.length && content[lineEnd] !== "\n") lineEnd++;
                                        const inclEnd = lineEnd < content.length ? lineEnd + 1 : lineEnd;
                                        cutText = content.slice(lineStart, inclEnd);
                                        removeStart = lineStart;
                                        removeEnd = inclEnd;
                                    }} else {{
                                        cutText = content.slice(selStart, selEnd);
                                        removeStart = selStart;
                                        removeEnd = selEnd;
                                    }}
                                    ev.preventDefault();
                                    // Same reason as the Ctrl+V branch
                                    // above: stop Monaco's native
                                    // copy/cut keybinding from running
                                    // with empty clipboardData on wry.
                                    ev.stopImmediatePropagation();
                                    if (key === "x" && cutText.length > 0) {{
                                        // Route through the bridge's
                                        // `replaceRange` which uses
                                        // `editor.executeEdits` — that
                                        // pushes the deletion onto
                                        // Monaco's undo stack, so
                                        // Ctrl+Z restores the cut text
                                        // (the previous setContent
                                        // path reset the model and
                                        // wiped undo history).
                                        try {{ handle.replaceRange(removeStart, removeEnd, ""); }}
                                        catch (e) {{ console.warn("operon: cut replaceRange threw", e); }}
                                    }}
                                    if (cutText.length > 0) {{
                                        try {{ console.log("operon: clipboard-write dispatch", "len", cutText.length); }} catch (_) {{}}
                                        safeSend({{type:"clipboard-write", text: cutText}});
                                    }}
                                    return;
                                }}
                                let action = null;
                                if (key === "s" && !ev.shiftKey) action = "save";
                                else if (key === "k" && !ev.shiftKey) action = "linkpicker";
                                else if (key === "i" && ev.shiftKey) action = "imagepicker";
                                if (!action) return;
                                ev.preventDefault();
                                ev.stopPropagation();
                                safeSend({{type:"keyaction", action}});
                                return;
                            }}
                            // Plans-Phase-9-monaco-desktop: trigger-char
                            // shortcuts for the link picker. `[[` (the
                            // second `[` after an existing one) and `@`
                            // at a non-word boundary both open the same
                            // picker the meta-K shortcut does. The
                            // trigger character is consumed here so
                            // accepting the picker yields a clean
                            // splice (no leftover `[[` or `@` in the
                            // body); on dismissal the user just lost
                            // those keystrokes — same trade-off Notion
                            // and Slack make for `@`.
                            if (ev.key === "[" || ev.key === "@") {{
                                let snap, content, cursor, prevChar;
                                try {{
                                    snap = handle.snapshot();
                                    content = handle.getContent();
                                    cursor = (snap && typeof snap.cursor === "number") ? snap.cursor : content.length;
                                    prevChar = cursor > 0 ? content[cursor - 1] : "";
                                }} catch (e) {{ return; }}
                                let shouldTrigger = false;
                                let deleteBefore = 0;
                                if (ev.key === "[" && prevChar === "[") {{
                                    shouldTrigger = true;
                                    deleteBefore = 1;
                                }} else if (ev.key === "@") {{
                                    // Boundary check: no prev char, or
                                    // prev is whitespace / newline /
                                    // start-of-line punctuation. This
                                    // keeps `name@example.com` from
                                    // accidentally firing the picker.
                                    if (cursor === 0 || /[\s(\[{{<,;:]/.test(prevChar)) {{
                                        shouldTrigger = true;
                                    }}
                                }}
                                if (!shouldTrigger) return;
                                ev.preventDefault();
                                ev.stopPropagation();
                                if (deleteBefore > 0) {{
                                    const next = content.slice(0, cursor - deleteBefore) + content.slice(cursor);
                                    suppress = true;
                                    try {{ handle.setContent(next); }} finally {{ suppress = false; }}
                                    try {{
                                        handle.restore({{
                                            cursor: cursor - deleteBefore,
                                            selection: null,
                                            scroll: (snap && snap.scroll) || 0,
                                        }});
                                    }} catch (e) {{}}
                                    safeSend({{type:"change", value: next, source: "trigger"}});
                                }}
                                safeSend({{type:"keyaction", action: "linkpicker"}});
                                return;
                            }}
                        }};
                        window.addEventListener("keydown", onKey, true);
                        safeSend({{type:"mounted"}});
                        try {{ console.log("operon: monaco recv loop START", '{host_id}'); }} catch (e) {{}}
                        while (true) {{
                            const msg = await dioxus.recv();
                            try {{ console.log("operon: monaco recv", '{host_id}', msg && msg.type); }} catch (e) {{}}
                            if (!msg || typeof msg !== "object") continue;
                            switch (msg.type) {{
                                case "setContent":
                                    suppress = true;
                                    try {{ handle.setContent(typeof msg.value === "string" ? msg.value : ""); }}
                                    finally {{ suppress = false; }}
                                    try {{ if (typeof handle.layout === "function") handle.layout(); }} catch (e) {{}}
                                    break;
                                case "splice":
                                    {{
                                        const state = handle.snapshot();
                                        const old = handle.getContent();
                                        const text = typeof msg.text === "string" ? msg.text : "";
                                        const pos = Math.min(state.cursor || 0, old.length);
                                        const next = old.slice(0, pos) + text + old.slice(pos);
                                        // Diagnostic: the user has seen
                                        // splice messages logged from Rust
                                        // but no change in Monaco. Surface
                                        // the JS-side state too so we can
                                        // tell if the insert happened.
                                        try {{
                                            console.log("operon: splice", {{
                                                hostId: '{host_id}',
                                                cursor: state.cursor,
                                                oldLen: old.length,
                                                textLen: text.length,
                                                pos,
                                                nextLen: next.length,
                                                preview: next.slice(Math.max(0, pos-10), pos+text.length+10),
                                            }});
                                        }} catch (e) {{}}
                                        suppress = true;
                                        try {{ handle.setContent(next); }} finally {{ suppress = false; }}
                                        handle.restore({{
                                            cursor: pos + text.length,
                                            selection: null,
                                            scroll: state.scroll || 0,
                                        }});
                                        // Manually emit the change so
                                        // Rust mirrors the new content.
                                        safeSend({{type:"change", value:next, source:"splice"}});
                                    }}
                                    break;
                                case "focus":
                                    handle.dispatch("Focus");
                                    break;
                                case "revealLine":
                                    {{
                                        const n = (typeof msg.value === "number" && msg.value > 0)
                                            ? Math.floor(msg.value)
                                            : 1;
                                        handle.dispatch("RevealLine:" + n);
                                    }}
                                    break;
                                case "dispose":
                                    window.removeEventListener("keydown", onKey, true);
                                    handle.dispose();
                                    delete window.__operon_monaco_handles['{host_id}'];
                                    return;
                            }}
                        }}
                    }} catch (e) {{
                        try {{ dioxus.send({{type:"error", message:String(e && e.message || e)}}); }} catch (_) {{}}
                    }}
                }})();"#,
                host_id = host_id,
                language_id_json = language_id_json,
                initial_content_json = initial_content_json,
            );
            document::eval(&script)
        });

        // Plans-Phase-9-monaco-desktop (rev 1, fix): expose this mount's
        // channel handle to the parent. We MUST overwrite any stale
        // value the parent's signal holds — when this `MonacoEditorHost`
        // re-mounts (e.g. after a tab switch or Edit↔Split toggle, or
        // even just after a hot reload during dev), `use_hook` mints a
        // new `eval_handle` but the parent's `monaco_channel` Signal
        // persists with the dead eval from the previous mount. The
        // earlier `if peek().is_none()` guard kept that stale value,
        // so paste-image / wikilink splices silently dropped because
        // they were posting into an Eval whose JS recv loop had ended.
        // `use_hook` runs once per mount so this still avoids extra
        // writes on every render.
        if let Some(mut sink) = channel_sink {
            let host_id_for_channel = host_id.clone();
            use_hook(move || {
                sink.set(Some(MonacoChannel {
                    eval: eval_handle,
                    host_id: host_id_for_channel,
                }));
            });
        }

        // Plans-Phase-9-monaco-desktop (rev 19): gate the content push on
        // the bootstrap script's `{type:"mounted"}` handshake. Even though
        // dioxus-desktop's eval bridge buffers Rust→JS messages in a
        // per-channel `pending` array, in practice an `eval.send` fired
        // during the render body before the bootstrap's `await dioxus.recv()`
        // loop starts gets dropped — see the comment on `MonacoChannel` at
        // the top of this file. The bootstrap already announces readiness
        // via `dioxus.send({type:"mounted"})`; we now listen for it,
        // flip a Signal, and only push setContent once the recv loop is
        // live. When the Signal flips, the render body re-runs and the
        // pending content (loaded from disk by `on_select_note`) finally
        // reaches Monaco.
        //
        // Earlier revs tried `use_effect` (rev 16), a mirror-Signal-with-
        // effect (rev 17), and the inline non-reactive Rc<RefCell> push
        // (rev 18). Rev 18 fixed the tab-switch case (where Monaco is
        // already mounted) but not the cold-reopen case (where the load
        // fires before mount completes).
        use std::cell::RefCell;
        use std::rc::Rc;
        let last_pushed: Rc<RefCell<String>> =
            use_hook(|| Rc::new(RefCell::new(content.clone())));
        let mut monaco_ready: Signal<bool> = use_signal(|| false);
        // Diagnostic: trace each render so we can tell whether the
        // parent's `content` prop is actually being threaded through
        // when Tab.content is written from an async paste handler.
        eprintln!(
            "operon: MonacoEditorHost render: ready={} content_len={} last_pushed_len={}",
            *monaco_ready.read(),
            content.len(),
            last_pushed.borrow().len()
        );
        // Mirror the private readiness flag into the parent's optional
        // sink so `LocalNoteEditor` can gate `MonacoChannel.splice` on
        // the `{type:"mounted"}` handshake. Same drop-before-recv-starts
        // hazard the existing `setContent` push handles.
        if let Some(mut rsink) = ready_sink {
            let live = *monaco_ready.read();
            if *rsink.peek() != live {
                rsink.set(live);
            }
        }
        {
            let ready = *monaco_ready.read();
            let needs_push = ready && *last_pushed.borrow() != content;
            if needs_push {
                eprintln!(
                    "operon: MonacoEditorHost prop-mirror PUSH setContent (len {})",
                    content.len()
                );
                // Two-pronged push for max reliability:
                //   1. Send through the bootstrap recv loop (the proven
                //      path on paper). If Dioxus drops it (we have
                //      evidence the recv queue is unreliable across
                //      multiple sends from the same eval handle), the
                //      backup below fires on the same render.
                //   2. Fresh `document::eval` script that grabs the
                //      stored `window.__operon_monaco_handles[hostId]`
                //      and calls `handle.setContent` directly. No
                //      queue, no recv loop dependency. We tag a global
                //      suppress flag for this host so the resulting
                //      onChange doesn't bounce back as user input.
                match eval_handle.send(serde_json::json!({
                    "type": "setContent",
                    "value": content.clone(),
                })) {
                    Ok(()) => eprintln!("operon: prop-mirror eval.send OK"),
                    Err(e) => eprintln!("operon: prop-mirror eval.send Err: {e:?}"),
                }
                let host_id_json = serde_json::to_string(&host_id)
                    .unwrap_or_else(|_| String::from("\"\""));
                let content_json = serde_json::to_string(&content)
                    .unwrap_or_else(|_| String::from("\"\""));
                let direct_script = format!(
                    "(function() {{ \
                        const hid = {host_id_json}; \
                        const h = (window.__operon_monaco_handles || {{}})[hid]; \
                        if (!h) {{ console.warn('operon: direct setContent: no handle for', hid); return; }} \
                        window.__operon_suppress_change = window.__operon_suppress_change || {{}}; \
                        window.__operon_suppress_change[hid] = true; \
                        try {{ h.setContent({content_json}); console.log('operon: direct setContent OK', hid, 'len', {len}); }} \
                        catch (e) {{ console.warn('operon: direct setContent threw', hid, e); }} \
                        finally {{ window.__operon_suppress_change[hid] = false; }} \
                        try {{ if (typeof h.layout === 'function') h.layout(); }} catch (e) {{}} \
                    }})();",
                    len = content.len(),
                );
                let _ = document::eval(&direct_script);
                *last_pushed.borrow_mut() = content.clone();
            }
        }

        // Focus request: explorer Enter (and click) writes the target note
        // id into the app-scope `RequestEditorFocus` signal. We only react
        // when both the request matches our note id AND Monaco has mounted
        // — readiness is gated by the `{type:"mounted"}` handshake. The
        // wasm path does the same dispatch via `EditorCommand::Focus`
        // (editor_host.rs:605-624); this is the desktop mirror.
        //
        // Two-pronged dispatch — same justification as the prop-mirror
        // setContent path above: the eval recv queue is unreliable across
        // multiple sends from the same handle, so we *also* fire a direct
        // `document::eval` script that grabs `window.__operon_monaco_handles`
        // and calls `editor.focus()` straight away. Either path landing is
        // enough for the caret to move into Monaco.
        let crate::editor::RequestEditorFocus(focus_request) = use_context();
        {
            let mut focus_request_setter = focus_request;
            let note_id_for_focus = note_id.clone();
            let host_id_for_focus = host_id.clone();
            let eval_for_focus = eval_handle;
            use_effect(move || {
                let ready = *monaco_ready.read();
                let req = focus_request_setter.read().clone();
                let Some(target) = req else { return };
                if !ready || target != note_id_for_focus {
                    return;
                }
                let _ = eval_for_focus.send(serde_json::json!({"type":"focus"}));
                let host_id_json = serde_json::to_string(&host_id_for_focus)
                    .unwrap_or_else(|_| String::from("\"\""));
                let direct_script = format!(
                    "(function() {{ \
                        const hid = {host_id_json}; \
                        const h = (window.__operon_monaco_handles || {{}})[hid]; \
                        if (!h) {{ console.warn('operon: direct focus: no handle for', hid); return; }} \
                        try {{ h.dispatch('Focus'); }} catch (e) {{ console.warn('operon: direct focus dispatch threw', hid, e); }} \
                    }})();",
                );
                let _ = document::eval(&direct_script);
                focus_request_setter.set(None);
            });
        }

        // Reveal-line request: a click in the LocalSearch panel writes
        // `(note_id, line)` into this signal. Fire whenever the signal is
        // touched AND Monaco is mounted (`monaco_ready`). The mount-handshake
        // path on first open is covered by the same effect because reading
        // both signals subscribes us to either one transitioning.
        let crate::editor::RequestEditorRevealLine(reveal_request) = use_context();
        {
            let mut reveal_request_setter = reveal_request;
            let note_id_for_reveal = note_id.clone();
            let eval_for_reveal = eval_handle;
            use_effect(move || {
                let ready = *monaco_ready.read();
                let req = reveal_request_setter.read().clone();
                let Some((target_note, line)) = req else { return };
                if !ready || target_note != note_id_for_reveal {
                    return;
                }
                let _ = eval_for_reveal
                    .send(serde_json::json!({"type":"revealLine","value":line}));
                reveal_request_setter.set(None);
            });
        }

        // Drive the JS → Rust channel. Each `change` becomes an
        // `on_change` invocation matching the wasm path's contract.
        // The `mounted` handshake flips `monaco_ready` so the prop-mirror
        // above can finally fire its first push.
        let on_change_for_loop = on_change;
        let mut eval_for_loop = eval_handle;
        // Cursor-preservation guard: when Monaco is the source of a
        // change (user typed, or the in-JS splice path re-emitted), we
        // stamp `last_pushed` to the freshly-typed value before letting
        // the on_change ripple through tabs.set_content. The next render
        // sees `*last_pushed.borrow() == content`, so the prop-mirror
        // above skips its setContent push — and Monaco's caret stays
        // where the user left it instead of collapsing to position 0.
        let last_pushed_for_loop = last_pushed.clone();
        let host_id_for_loop = host_id.clone();
        use_future(move || {
            let last_pushed_for_loop = last_pushed_for_loop.clone();
            let host_id_for_loop = host_id_for_loop.clone();
            async move {
            loop {
                let msg: serde_json::Value = match eval_for_loop.recv().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let kind = msg.get("type").and_then(|v| v.as_str());
                match kind {
                    Some("mounted") => {
                        eprintln!("operon: monaco mounted handshake received (host scope)");
                        monaco_ready.set(true);
                    }
                    Some("change") => {
                        let value = msg
                            .get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        *last_pushed_for_loop.borrow_mut() = value.clone();
                        on_change_for_loop.call(value);
                    }
                    Some("keyaction") => {
                        if let Some(handler) = on_action {
                            let action = msg
                                .get("action")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            handler.call(action);
                        }
                    }
                    Some("paste") => {
                        eprintln!("operon: monaco paste arm hit (host {})", host_id_for_loop);
                        // Cmd/Ctrl+V on a Monaco surface. Because
                        // wry/WebKitGTK silently drops text from
                        // `clipboardData.getData`, Monaco's native
                        // paste produces nothing on Linux. Bypass by
                        // reading the OS clipboard via arboard and
                        // splicing the text into Monaco directly via
                        // a fresh `document::eval` (the same proven
                        // path `MonacoChannel::splice` uses — bypasses
                        // the unreliable bootstrap recv queue).
                        // Image-only clipboards return
                        // `ContentNotAvailable` from arboard's text
                        // reader; we silently skip and let the
                        // document-level image-paste handler in
                        // LocalNoteEditor / artifact-view claim the
                        // bytes via `read_clipboard_image_png`.
                        let text = match crate::util::clipboard::read_clipboard_text() {
                            Ok(t) if !t.is_empty() => {
                                eprintln!("operon: monaco paste clipboard text len={}", t.len());
                                t
                            }
                            Ok(_) => {
                                eprintln!("operon: monaco paste clipboard returned EMPTY string");
                                continue;
                            }
                            Err(e) => {
                                eprintln!("operon: monaco paste clipboard read failed: {e}");
                                continue;
                            }
                        };
                        let host_id_json = serde_json::to_string(&host_id_for_loop)
                            .unwrap_or_else(|_| String::from("\"\""));
                        let text_json = serde_json::to_string(&text)
                            .unwrap_or_else(|_| String::from("\"\""));
                        let direct_script = format!(
                            "(function() {{ \
                                const hid = {host_id_json}; \
                                const h = (window.__operon_monaco_handles || {{}})[hid]; \
                                if (!h) {{ console.warn('operon: paste-text: no handle for', hid); return; }} \
                                try {{ \
                                    const state = h.snapshot(); \
                                    const old = h.getContent(); \
                                    const text = {text_json}; \
                                    let s, e; \
                                    if (state && state.selection) {{ \
                                        s = state.selection[0]; e = state.selection[1]; \
                                    }} else {{ \
                                        const pos = Math.min((state && state.cursor) || 0, old.length); \
                                        s = pos; e = pos; \
                                    }} \
                                    if (typeof h.replaceRange === 'function') {{ \
                                        h.replaceRange(s, e, text); \
                                    }} else {{ \
                                        /* legacy bridge fallback — non-undoable */ \
                                        const next = old.slice(0, s) + text + old.slice(e); \
                                        h.setContent(next); \
                                        try {{ h.restore({{cursor: s + text.length, selection: null, scroll: (state && state.scroll) || 0}}); }} catch (e2) {{}} \
                                    }} \
                                    console.log('operon: paste-text OK', hid, 'len', text.length); \
                                }} catch (e) {{ \
                                    console.warn('operon: paste-text threw', hid, e); \
                                }} \
                            }})();",
                        );
                        let _ = document::eval(&direct_script);
                    }
                    Some("clipboard-write") => {
                        // Cmd/Ctrl+C or Cmd/Ctrl+X: the bootstrap
                        // already derived the selected (or current-
                        // line) text JS-side and, for cut, spliced it
                        // out of Monaco. All that's left is writing
                        // the bytes to the OS clipboard via arboard
                        // — Monaco's native copy/cut path is broken
                        // in WebKitGTK the same way paste is.
                        let text = msg
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if text.is_empty() {
                            eprintln!("operon: clipboard-write skipped (empty text)");
                            continue;
                        }
                        match crate::util::clipboard::write_clipboard_text(&text) {
                            Ok(()) => eprintln!(
                                "operon: clipboard-write OK len={}",
                                text.len()
                            ),
                            Err(e) => eprintln!("operon: clipboard write failed: {e}"),
                        }
                    }
                    Some("error") => {
                        let m = msg.get("message").and_then(|v| v.as_str()).unwrap_or("");
                        eprintln!("operon: monaco bridge error: {m}");
                    }
                    _ => {}
                }
            }
            }
        });

        // Tear down Monaco on unmount. Eval is Copy, so capturing it in
        // the drop closure is fine.
        let eval_for_dispose = eval_handle;
        use_drop(move || {
            let _ = eval_for_dispose.send(serde_json::json!({"type":"dispose"}));
        });

        return rsx! {
            div {
                // Plans-Phase-9-monaco-desktop (rev 8): absolute-inset
                // against the positioned `LocalNoteEditor` wrapping
                // div (which is itself absolute-inset against
                // `.operon-main-body` / `.operon-local-split-edit`).
                // Each layer fills its parent independent of any flex
                // calc — Monaco's host always has deterministic
                // dimensions equal to the outer positioned ancestor.
                style: "position: absolute; inset: 0;",
                div {
                    id: "{host_id}",
                    class: "operon-monaco-host",
                    "data-monaco-host": "{note_id_attr}",
                    "data-monaco-language": "{language_attr}",
                    "data-stub": "false",
                    style: "position: absolute; inset: 0;",
                }
            }
        };
    }

    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::editor::{
            BackendInit, EditorBackend, EditorCommand, MonacoBackend, RequestEditorFocus,
            RequestEditorRevealLine,
        };
        use crate::theme::{editor_theme, Theme};

        let theme: Signal<Theme> = use_context();
        let backend: Signal<Rc<RefCell<MonacoBackend>>> =
            use_signal(|| Rc::new(RefCell::new(MonacoBackend::new())));
        // Plans-Phase-2-editor-auto-focus: read the app-scope focus-request
        // signal so we can grant focus when our note id matches.
        let RequestEditorFocus(mut focus_request) = use_context();
        let RequestEditorRevealLine(mut reveal_request) = use_context();

        // Mount once on first render with the host element. Re-runs are safe — MonacoBackend
        // tracks `disposed` internally and stale calls are no-ops.
        let host_id = format!("operon-monaco-host-{note_id}");
        let host_id_for_effect = host_id.clone();
        let initial_content = content.clone();
        let language_for_effect = language.clone();
        let theme_blob = editor_theme::monaco_blob(&theme.read());
        let note_id_for_effect = note_id.clone();

        use_effect(move || {
            let host_id = host_id_for_effect.clone();
            let initial_content = initial_content.clone();
            let language = language_for_effect.clone();
            let theme_blob = theme_blob.clone();
            let backend = backend.clone();
            let on_change = on_change;
            let note_id_capture = note_id_for_effect.clone();
            spawn(async move {
                let Some(window) = web_sys::window() else { return };
                let Some(doc) = window.document() else { return };
                // Dioxus may not have flushed the host element yet; one rAF is sufficient.
                let Some(target) = doc.get_element_by_id(&host_id) else { return };
                let init = BackendInit {
                    language,
                    initial_content,
                    theme: theme_blob,
                    read_only: false,
                };
                let bk = backend.read().clone();
                let mount_res = bk.borrow_mut().mount(target, init).await;
                if mount_res.is_ok() {
                    bk.borrow().on_change(Box::new(move |new_content| {
                        on_change.call(new_content);
                    }));
                    // Plans-Phase-2-editor-auto-focus: if our note is the one
                    // requesting focus, dispatch and clear the signal.
                    let wants_focus = focus_request
                        .read()
                        .as_deref()
                        .map(|id| id == note_id_capture.as_str())
                        .unwrap_or(false);
                    if wants_focus {
                        bk.borrow().dispatch(EditorCommand::Focus);
                        focus_request.set(None);
                    }
                    // Reveal-line: same idiom — dispatch + clear if the
                    // pending request targets our note id.
                    let pending_reveal = reveal_request.read().clone();
                    if let Some((target, line)) = pending_reveal {
                        if target == note_id_capture.as_str() {
                            bk.borrow().dispatch(EditorCommand::RevealLine(line));
                            reveal_request.set(None);
                        }
                    }
                }
            });
        });

        // Plans-Phase-7-tab-activation-focus: a second `use_effect` whose
        // only input is `focus_request` so re-activating an already-mounted
        // tab (search-result click, wikilink jump, explorer re-click) also
        // grants focus. The mount-effect above only fires on first mount;
        // this one fires whenever the focus request signal changes.
        let backend_for_activation = backend;
        let note_id_for_activation = note_id.clone();
        use_effect(move || {
            let wants = focus_request
                .read()
                .as_deref()
                .map(|id| id == note_id_for_activation.as_str())
                .unwrap_or(false);
            if !wants {
                return;
            }
            let bk = backend_for_activation.read().clone();
            // Skip if the backend isn't mounted yet — the mount-effect's
            // async branch will take care of it on first mount.
            bk.borrow().dispatch(EditorCommand::Focus);
            focus_request.set(None);
        });

        // Reveal-line activation: same pattern as focus, fires on signal
        // change so a click into an already-mounted tab still scrolls.
        let backend_for_reveal = backend;
        let note_id_for_reveal = note_id.clone();
        use_effect(move || {
            let pending = reveal_request.read().clone();
            let Some((target, line)) = pending else { return };
            if target != note_id_for_reveal {
                return;
            }
            let bk = backend_for_reveal.read().clone();
            bk.borrow().dispatch(EditorCommand::RevealLine(line));
            reveal_request.set(None);
        });

        // Plans-Phase-7-clear-focus-on-dispose: when this editor host
        // unmounts (tab close, route change), clear the focus request if
        // it still points at our note id so the next mount can't pick up
        // a stale request.
        let note_id_for_dispose = note_id.clone();
        let mut focus_request_for_dispose = focus_request;
        let _drop_guard = use_drop(move || {
            let bk = backend.read().clone();
            bk.borrow_mut().dispose();
            let stale = focus_request_for_dispose
                .read()
                .as_deref()
                .map(|id| id == note_id_for_dispose.as_str())
                .unwrap_or(false);
            if stale {
                focus_request_for_dispose.set(None);
            }
        });

        return rsx! {
            div {
                id: "{host_id}",
                class: "operon-monaco-host",
                "data-monaco-host": "{note_id_attr}",
                "data-monaco-language": "{language_attr}",
                style: "width: 100%; height: 100%; min-height: 300px;",
            }
        };
    }
}
