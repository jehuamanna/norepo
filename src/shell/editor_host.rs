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
#[derive(Clone, Copy)]
pub struct MonacoChannel {
    eval: document::Eval,
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

    /// Insert `text` at Monaco's current caret position. JS-side
    /// computes the caret from `handle.snapshot()`, so the Rust caller
    /// doesn't need to round-trip the cursor offset. Falls back to
    /// end-of-buffer when the cursor is past the content (Monaco
    /// snapshots `getOffsetAt(getPosition())`, which clamps to model
    /// length).
    pub fn splice(&self, text: &str) {
        let _ = self
            .eval
            .send(serde_json::json!({"type":"splice","text":text}));
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
        let host_id = format!("operon-monaco-host-{note_id}");
        let initial_content = content.clone();
        let language_id = language.id.to_string();
        // Plans-Phase-9-monaco-desktop (rev 3): visible mount status so
        // we can debug "Monaco never appears" without having to open
        // the webview's devtools. The recv loop flips this to "mounted"
        // on success or "error: <msg>" on bridge / mount failure.
        let mut mount_status: Signal<String> =
            use_signal(|| "Loading Monaco…".to_string());
        let mut mounted_flag: Signal<bool> = use_signal(|| false);

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
                        dioxus.send({{type:"diag", phase:"start", origin:String(window.location && window.location.origin)}});
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
                                dioxus.send({{type:"diag", phase:"imported"}});
                            }} catch (impErr) {{
                                dioxus.send({{type:"error", message:"import failed: "+String(impErr && impErr.message || impErr)}});
                                return;
                            }}
                        }}
                        if (!window.operonBridge) {{
                            dioxus.send({{type:"error", message:"bridge not loaded"}});
                            return;
                        }}
                        // Plans-Phase-9-monaco-desktop (rev 10): wait
                        // for the host element to (a) flush into the
                        // DOM AND (b) get a non-zero clientWidth /
                        // clientHeight before mounting Monaco. In
                        // Split mode the parent flex chain takes
                        // multiple animation frames to resolve, and
                        // mounting against a 0x0 host leaves Monaco
                        // technically present but visually empty
                        // (its automaticLayout ResizeObserver
                        // sometimes misses the 0 -> N transition).
                        // We poll up to ~3 seconds; if we still don't
                        // have size, mount anyway and log it so the
                        // diag tells us a CSS regression survived.
                        let target = document.getElementById('{host_id}');
                        let attempts = 0;
                        while (!target && attempts < 60) {{
                            await new Promise(r => setTimeout(r, 33));
                            target = document.getElementById('{host_id}');
                            attempts++;
                        }}
                        if (!target) {{
                            dioxus.send({{type:"error", message:"host element not found"}});
                            return;
                        }}
                        let sizeAttempts = 0;
                        while ((target.clientWidth === 0 || target.clientHeight === 0) && sizeAttempts < 90) {{
                            await new Promise(r => setTimeout(r, 33));
                            sizeAttempts++;
                        }}
                        dioxus.send({{
                            type:"diag",
                            phase:"target-found",
                            origin: "w="+target.clientWidth+" h="+target.clientHeight+" tries="+sizeAttempts,
                        }});
                        const handle = await window.operonBridge.mount(target, {{
                            kind: "monaco",
                            languageId: {language_id_json},
                            content: {initial_content_json},
                            theme: "vs",
                            readOnly: false,
                        }});
                        dioxus.send({{type:"diag", phase:"mount-returned"}});
                        window.__operon_monaco_handles = window.__operon_monaco_handles || {{}};
                        window.__operon_monaco_handles['{host_id}'] = handle;
                        // Plans-Phase-9-monaco-desktop (rev 7): force a
                        // re-layout once the parent flex has settled.
                        // Monaco's `automaticLayout: true` uses a
                        // ResizeObserver that occasionally doesn't fire
                        // when the host element gets its final size
                        // *after* `editor.create()` ran (Split mode
                        // hits this — host is 0x0 at mount, then
                        // resolves to 50% of the column). Two
                        // setTimeouts at increasing delays cover both
                        // immediate (next-frame) and slightly-deferred
                        // (post-flex-settle) cases.
                        const relayout = () => {{
                            try {{
                                if (!handle || !handle.layout) return;
                                handle.layout();
                                dioxus.send({{
                                    type:"diag",
                                    phase:"relayout",
                                    origin: "w="+target.clientWidth+" h="+target.clientHeight,
                                }});
                            }} catch (e) {{}}
                        }};
                        setTimeout(relayout, 0);
                        setTimeout(relayout, 100);
                        setTimeout(relayout, 500);
                        // Suppress change events fired by programmatic
                        // setContent so Rust doesn't see its own write
                        // bounce back as user input.
                        let suppress = false;
                        handle.onChange((c) => {{
                            if (suppress) return;
                            dioxus.send({{type:"change", value:c}});
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
                            if (!meta) return;
                            const key = ev.key && ev.key.toLowerCase();
                            let action = null;
                            if (key === "s" && !ev.shiftKey) action = "save";
                            else if (key === "k" && !ev.shiftKey) action = "linkpicker";
                            else if (key === "i" && ev.shiftKey) action = "imagepicker";
                            if (!action) return;
                            ev.preventDefault();
                            ev.stopPropagation();
                            dioxus.send({{type:"keyaction", action}});
                        }};
                        window.addEventListener("keydown", onKey, true);
                        dioxus.send({{type:"mounted"}});
                        while (true) {{
                            const msg = await dioxus.recv();
                            if (!msg || typeof msg !== "object") continue;
                            switch (msg.type) {{
                                case "setContent":
                                    suppress = true;
                                    try {{ handle.setContent(typeof msg.value === "string" ? msg.value : ""); }}
                                    finally {{ suppress = false; }}
                                    break;
                                case "splice":
                                    {{
                                        const state = handle.snapshot();
                                        const old = handle.getContent();
                                        const text = typeof msg.text === "string" ? msg.text : "";
                                        const pos = Math.min(state.cursor || 0, old.length);
                                        const next = old.slice(0, pos) + text + old.slice(pos);
                                        suppress = true;
                                        try {{ handle.setContent(next); }} finally {{ suppress = false; }}
                                        handle.restore({{
                                            cursor: pos + text.length,
                                            selection: null,
                                            scroll: state.scroll || 0,
                                        }});
                                        // Manually emit the change so
                                        // Rust mirrors the new content.
                                        dioxus.send({{type:"change", value:next}});
                                    }}
                                    break;
                                case "focus":
                                    handle.dispatch("Focus");
                                    break;
                                case "dispose":
                                    window.removeEventListener("keydown", onKey, true);
                                    handle.dispose();
                                    delete window.__operon_monaco_handles['{host_id}'];
                                    return;
                            }}
                        }}
                    }} catch (e) {{
                        dioxus.send({{type:"error", message:String(e && e.message || e)}});
                    }}
                }})();"#,
                host_id = host_id,
                language_id_json = language_id_json,
                initial_content_json = initial_content_json,
            );
            document::eval(&script)
        });

        // Plans-Phase-9-monaco-desktop (rev 1): expose a channel handle
        // to the parent (if it asked for one). The channel is just the
        // Eval (Copy) wrapped in a typed setContent/focus surface; any
        // messages enqueued before the JS-side recv loop is ready buffer
        // and dispatch when it catches up.
        if let Some(mut sink) = channel_sink {
            if sink.peek().is_none() {
                sink.set(Some(MonacoChannel { eval: eval_handle }));
            }
        }

        // Drive the JS → Rust channel. Each `change` becomes an
        // `on_change` invocation matching the wasm path's contract.
        let on_change_for_loop = on_change;
        let mut eval_for_loop = eval_handle;
        use_future(move || async move {
            loop {
                let msg: serde_json::Value = match eval_for_loop.recv().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let kind = msg.get("type").and_then(|v| v.as_str());
                match kind {
                    Some("change") => {
                        let value = msg
                            .get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        eprintln!("operon: monaco change len={}", value.len());
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
                    Some("mounted") => {
                        eprintln!("operon: monaco mounted");
                        mounted_flag.set(true);
                        mount_status.set("mounted".to_string());
                    }
                    Some("error") => {
                        let m = msg.get("message").and_then(|v| v.as_str()).unwrap_or("");
                        eprintln!("operon: monaco bridge error: {m}");
                        mount_status.set(format!("error: {m}"));
                    }
                    Some("diag") => {
                        let phase = msg.get("phase").and_then(|v| v.as_str()).unwrap_or("");
                        let origin = msg.get("origin").and_then(|v| v.as_str()).unwrap_or("");
                        eprintln!("operon: monaco diag phase={phase} origin={origin}");
                        if !origin.is_empty() {
                            mount_status.set(format!("phase={phase} origin={origin}"));
                        } else {
                            mount_status.set(format!("phase={phase}"));
                        }
                    }
                    _ => {}
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
                if !*mounted_flag.read() {
                    div {
                        class: "operon-monaco-status",
                        "data-testid": "monaco-mount-status",
                        style: "position: absolute; top: 8px; left: 8px; padding: 6px 10px; \
                                background: rgba(255, 235, 59, 0.85); color: #1a1a1a; \
                                font-family: monospace; font-size: 12px; border-radius: 4px; \
                                z-index: 10; max-width: 90%; white-space: pre-wrap;",
                        "{mount_status.read()}"
                    }
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
        };
        use crate::theme::{editor_theme, Theme};

        let theme: Signal<Theme> = use_context();
        let backend: Signal<Rc<RefCell<MonacoBackend>>> =
            use_signal(|| Rc::new(RefCell::new(MonacoBackend::new())));
        // Plans-Phase-2-editor-auto-focus: read the app-scope focus-request
        // signal so we can grant focus when our note id matches.
        let RequestEditorFocus(mut focus_request) = use_context();

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
