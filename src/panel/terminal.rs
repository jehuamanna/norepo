//! Panel "Terminal" tab — multi-session, tabbed PTY surface.
//!
//! The visible tab strip + body lives here; the descriptor list (id /
//! label / cwd) lives in [`super::TerminalsManager`] which is provided
//! as a `Signal<TerminalsManager>` via the app's context. That split
//! lets project-row context menus and the `+` button push new tabs
//! into the manager from anywhere in the tree without depending on the
//! native-only PTY plumbing here.
//!
//! ## Session lifetime
//!
//! Sessions are process-global, keyed by [`TerminalId`] (not by cwd),
//! and live in [`SESSIONS`]. Closing a tab calls [`kill_session`]
//! which drops the master PTY → child shell receives SIGHUP → exits.
//! The cleanup watcher (per spawn) removes the session from the map
//! once the child reaps. The bridge future also notices broadcast
//! closure (Ctrl-D / `exit`) and asks the manager to remove the tab,
//! so Ctrl-D in the shell closes the corresponding visual tab.
//!
//! Wasm gating: this whole module is native-only — `panel/mod.rs`
//! `cfg`-guards the export. The cross-platform `TerminalsManager`
//! lives in `panel/mod.rs` so context-providers work on both targets.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use dioxus::prelude::*;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;

use super::{TerminalDescriptor, TerminalId, TerminalsManager};
use crate::local_mode::desktop::LocalProjectRepo;
use crate::local_mode::explorer::{LocalProjectVersion, SelectedProject};

const XTERM_JS: &str = include_str!("../../assets/xterm/xterm.min.js");
const XTERM_CSS: &str = include_str!("../../assets/xterm/xterm.min.css");
const XTERM_FIT_JS: &str =
    include_str!("../../assets/xterm/xterm-addon-fit.min.js");

const SCROLLBACK_BYTES: usize = 256 * 1024;
const BROADCAST_CAP: usize = 4096;

fn xterm_theme_json_for(kind: crate::theme::ThemeKind) -> String {
    use crate::theme::ThemeKind;
    match kind {
        ThemeKind::Light => serde_json::json!({
            "background": "#ffffff",
            "foreground": "#3b3b3b",
            "cursor": "#3b3b3b",
            "cursorAccent": "#ffffff",
            "selectionBackground": "#add6ff",
            "selectionForeground": "#000000",
            "black": "#000000",
            "red": "#cd3131",
            "green": "#00bc00",
            "yellow": "#949800",
            "blue": "#0451a5",
            "magenta": "#bc05bc",
            "cyan": "#0598bc",
            "white": "#555555",
            "brightBlack": "#666666",
            "brightRed": "#cd3131",
            "brightGreen": "#14ce14",
            "brightYellow": "#b5ba00",
            "brightBlue": "#0451a5",
            "brightMagenta": "#bc05bc",
            "brightCyan": "#0598bc",
            "brightWhite": "#a5a5a5",
        })
        .to_string(),
        _ => serde_json::json!({
            "background": "#1e1e1e",
            "foreground": "#d4d4d4",
            "cursor": "#d4d4d4",
            "cursorAccent": "#1e1e1e",
            "selectionBackground": "#264f78",
            "black": "#000000",
            "red": "#cd3131",
            "green": "#0dbc79",
            "yellow": "#e5e510",
            "blue": "#2472c8",
            "magenta": "#bc3fbc",
            "cyan": "#11a8cd",
            "white": "#e5e5e5",
            "brightBlack": "#666666",
            "brightRed": "#f14c4c",
            "brightGreen": "#23d18b",
            "brightYellow": "#f5f543",
            "brightBlue": "#3b8eea",
            "brightMagenta": "#d670d6",
            "brightCyan": "#29b8db",
            "brightWhite": "#e5e5e5",
        })
        .to_string(),
    }
}

struct ShellSession {
    out_tx: broadcast::Sender<Vec<u8>>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

fn sessions() -> &'static Mutex<HashMap<TerminalId, Arc<ShellSession>>> {
    static MAP: OnceLock<Mutex<HashMap<TerminalId, Arc<ShellSession>>>> =
        OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Tear down a session — called when the user closes a tab.
/// Dropping the `MasterPty` here causes the slave end to close, the
/// shell child sees EOF/SIGHUP and exits, and the per-session cleanup
/// watcher (spawned in `get_or_create_session`) reaps the map entry.
pub fn kill_session(id: TerminalId) {
    let removed = {
        let mut map = sessions().lock().unwrap();
        map.remove(&id)
    };
    drop(removed);
}

fn resolve_shell_bin() -> PathBuf {
    if let Ok(s) = std::env::var("SHELL") {
        let p = PathBuf::from(&s);
        if p.is_absolute() {
            return p;
        }
    }
    for candidate in ["/bin/bash", "/usr/bin/bash", "/bin/zsh", "/usr/bin/zsh"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("/bin/sh")
}

fn fallback_cwd() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from("/")
}

fn get_or_create_session(
    id: TerminalId,
    cwd: &Path,
    shell_bin: &Path,
) -> Result<Arc<ShellSession>, String> {
    {
        let map = sessions().lock().unwrap();
        if let Some(s) = map.get(&id) {
            return Ok(s.clone());
        }
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut cmd = CommandBuilder::new(shell_bin.as_os_str());
    // -l: login shell so the user's full env (.bash_profile / .zprofile)
    // is sourced — gives nvm, asdf, pyenv etc. the same setup they'd
    // get in a real terminal.
    cmd.arg("-l");
    cmd.cwd(cwd.as_os_str());
    for (k, v) in std::env::vars_os() {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn failed: {e}"))?;
    let pid = child.process_id().unwrap_or(0);
    tracing::info!(
        target: "operon::panel_terminal",
        pid,
        cwd = %cwd.display(),
        shell = %shell_bin.display(),
        terminal_id = id.0,
        "spawned shell PTY child"
    );
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take_writer failed: {e}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("try_clone_reader failed: {e}"))?;

    let scrollback: Arc<Mutex<VecDeque<u8>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(SCROLLBACK_BYTES)));
    let (out_tx, _) = broadcast::channel::<Vec<u8>>(BROADCAST_CAP);

    let scrollback_for_reader = scrollback.clone();
    let out_tx_for_reader = out_tx.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    {
                        let mut sb = scrollback_for_reader.lock().unwrap();
                        let overflow = (sb.len() + chunk.len())
                            .saturating_sub(SCROLLBACK_BYTES);
                        for _ in 0..overflow {
                            sb.pop_front();
                        }
                        sb.extend(chunk.iter().copied());
                    }
                    let _ = out_tx_for_reader.send(chunk);
                }
                Err(_) => break,
            }
        }
    });

    std::thread::spawn(move || {
        let exit = child.wait();
        tracing::warn!(
            target: "operon::panel_terminal",
            ?exit,
            terminal_id = id.0,
            "shell PTY child exited; evicting session"
        );
        let mut map = sessions().lock().unwrap();
        map.remove(&id);
    });

    let session = Arc::new(ShellSession {
        out_tx,
        scrollback,
        writer: Arc::new(Mutex::new(writer)),
        master: Arc::new(Mutex::new(pair.master)),
    });
    sessions().lock().unwrap().insert(id, session.clone());
    Ok(session)
}

/// Resolve the active project's bound repo path, if any — used by the
/// `+` button to default a new tab to whatever the user is currently
/// working on, mirroring VS Code's "open terminal here" default.
fn active_project_cwd_and_label() -> (PathBuf, String) {
    let selected = try_consume_context::<SelectedProject>().map(|c| c.0);
    let project_repo =
        try_consume_context::<LocalProjectRepo>().map(|c| c.0);
    let pid = selected.as_ref().and_then(|s| *s.read());
    if let (Some(pid), Some(repo)) = (pid, project_repo.as_ref()) {
        if let Ok(rows) = repo.list() {
            if let Some(project) = rows.into_iter().find(|p| p.id == pid) {
                let label = project.name.clone();
                if let Some(p) = project.repo_path {
                    if p.is_dir() {
                        return (p, label);
                    }
                }
                // Project selected but no bound repo — keep its name as
                // the label, fall back to $HOME for the cwd.
                return (fallback_cwd(), label);
            }
        }
    }
    (fallback_cwd(), "shell".to_string())
}

#[component]
pub fn TerminalsView() -> Element {
    let mut manager: Signal<TerminalsManager> = use_context();
    // Touch the project version so the +-button's default cwd reflects
    // any inline rename / repo rebind that happened since last render.
    if let Some(v) = try_consume_context::<LocalProjectVersion>().map(|c| c.0)
    {
        let _ = v.read();
    }

    let snapshot = manager.read();
    let entries: Vec<TerminalDescriptor> = snapshot.iter().cloned().collect();
    let active = snapshot.active();
    drop(snapshot);

    let create_new = move |_| {
        let (cwd, label) = active_project_cwd_and_label();
        manager.write().create(label, cwd);
    };

    if entries.is_empty() {
        return rsx! {
            div {
                class: "operon-panel-terminals-empty",
                style: "display:flex; flex-direction:column; align-items:center; justify-content:center; height:100%; gap:0.75rem; color: var(--vscode-descriptionforeground, #6e6e6e);",
                div { style: "font-size: 0.9em;", "No terminals open." }
                button {
                    style: "padding: 4px 12px; font-size: 12px; cursor: pointer; border: 1px solid var(--vscode-panel-border, #cecece); background: var(--vscode-button-secondaryBackground, transparent); color: inherit; border-radius: 3px;",
                    onclick: create_new,
                    "+ New terminal"
                }
            }
        };
    }

    let active_descriptor = active.and_then(|id| {
        entries.iter().find(|t| t.id == id).cloned()
    });

    rsx! {
        div {
            class: "operon-panel-terminals",
            style: "display: flex; flex-direction: column; height: 100%; min-height: 0;",
            // Tab strip
            div {
                class: "operon-panel-terminals-strip",
                style: "flex: 0 0 auto; display: flex; align-items: stretch; min-height: 26px; background: var(--vscode-panelheader-background, var(--vscode-panel-background, #f3f3f3)); border-bottom: 1px solid var(--vscode-panel-border, #e1e1e1); overflow-x: auto;",
                for desc in entries.iter().cloned() {
                    {
                        let is_active = Some(desc.id) == active;
                        let id = desc.id;
                        let label = desc.label.clone();
                        let cwd_label = desc.cwd.display().to_string();
                        let title = format!("{label} — {cwd_label}");
                        let tab_bg = if is_active {
                            "var(--vscode-tab-activeBackground, var(--vscode-panel-background, #ffffff))"
                        } else {
                            "transparent"
                        };
                        let tab_color = if is_active {
                            "var(--vscode-tab-activeForeground, var(--vscode-editor-foreground, #1e1e1e))"
                        } else {
                            "var(--vscode-tab-inactiveForeground, var(--vscode-descriptionforeground, #6e6e6e))"
                        };
                        let border_bottom = if is_active {
                            "2px solid var(--vscode-focusBorder, #0090f1)"
                        } else {
                            "2px solid transparent"
                        };
                        rsx! {
                            div {
                                key: "{id.0}",
                                class: "operon-panel-terminals-tab",
                                "data-testid": "panel-terminal-tab",
                                "data-terminal-id": "{id.0}",
                                "data-active": if is_active { "true" } else { "false" },
                                title: "{title}",
                                style: "display: flex; align-items: center; gap: 6px; padding: 3px 8px 1px 10px; font-size: 12px; cursor: pointer; background: {tab_bg}; color: {tab_color}; border-bottom: {border_bottom}; user-select: none; white-space: nowrap;",
                                onclick: move |_| { manager.write().activate(id); },
                                span { "{label}" }
                                button {
                                    style: "border: none; background: transparent; cursor: pointer; padding: 0 4px; font-size: 14px; line-height: 1; color: inherit; opacity: 0.6;",
                                    title: "Close terminal",
                                    onclick: move |evt: Event<MouseData>| {
                                        evt.stop_propagation();
                                        kill_session(id);
                                        manager.write().close(id);
                                    },
                                    "\u{00d7}"
                                }
                            }
                        }
                    }
                }
                button {
                    class: "operon-panel-terminals-new",
                    "data-testid": "panel-terminal-new",
                    title: "New terminal",
                    style: "border: none; background: transparent; cursor: pointer; padding: 0 10px; font-size: 14px; line-height: 1; color: var(--vscode-descriptionforeground, #6e6e6e); align-self: center;",
                    onclick: create_new,
                    "+"
                }
            }
            // Body
            div {
                class: "operon-panel-terminals-body",
                style: "flex: 1 1 auto; min-height: 0; position: relative; overflow: hidden;",
                if let Some(desc) = active_descriptor {
                    ShellTerminalHost {
                        key: "{desc.id.0}",
                        terminal_id: desc.id,
                        cwd: desc.cwd.clone(),
                        label: desc.label.clone(),
                    }
                }
            }
        }
    }
}

#[component]
fn ShellTerminalHost(
    terminal_id: TerminalId,
    cwd: PathBuf,
    label: String,
) -> Element {
    let host_seq: u64 = use_hook(|| {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        SEQ.fetch_add(1, Ordering::Relaxed)
    });
    let host_id = format!("operon-panel-terminal-{host_seq}");

    let shell_bin = resolve_shell_bin();
    let cwd_label = cwd.display().to_string();
    let bin_label = shell_bin.display().to_string();

    let session_result: Result<Arc<ShellSession>, String> = use_hook({
        let cwd = cwd.clone();
        let shell_bin = shell_bin.clone();
        move || get_or_create_session(terminal_id, &cwd, &shell_bin)
    });

    let session = match session_result {
        Ok(s) => s,
        Err(e) => {
            return rsx! {
                div {
                    class: "operon-panel-terminal-error",
                    style: "padding: 1rem; font-family: ui-monospace, monospace; font-size: 12px; color: #f88; white-space: pre-wrap;",
                    "Could not start shell.\n\n"
                    "{bin_label} -l (cwd {cwd_label})\n{e}"
                }
            };
        }
    };

    let theme_signal = try_consume_context::<crate::theme::ThemeSignal>();
    let theme_kind = theme_signal
        .as_ref()
        .map(|t| t.read().kind)
        .unwrap_or(crate::theme::ThemeKind::Light);
    let xterm_theme_json = xterm_theme_json_for(theme_kind);

    if let Some(sig) = theme_signal {
        let host_id_for_theme = host_id.clone();
        let mut previous = use_signal(|| theme_kind);
        use_effect(move || {
            let current = sig.read().kind;
            if *previous.peek() == current {
                return;
            }
            previous.set(current);
            let theme_json = xterm_theme_json_for(current);
            let host_id_json =
                serde_json::to_string(&host_id_for_theme).unwrap_or_default();
            let script = format!(
                "(function() {{ const e = (window.__operon_terms || {{}})[{host_id_json}]; if (e && e.term) {{ try {{ e.term.options.theme = {theme_json}; }} catch (_) {{}} }} }})();"
            );
            let _ = document::eval(&script);
        });
    }

    // When the broadcast closes (PTY child exited, e.g. Ctrl-D /
    // `exit`), the bridge future asks the manager to remove this tab.
    // The manager lives in a context Signal, so we capture it here and
    // hand the future a closure that does the write.
    let mut manager: Signal<TerminalsManager> = use_context();
    let on_session_exit = move || {
        manager.write().close(terminal_id);
    };

    let host_id_for_future = host_id.clone();
    let session_for_future = session.clone();
    let xterm_theme_for_future = xterm_theme_json.clone();
    let label_for_future = label.clone();
    use_future(move || {
        let host_id = host_id_for_future.clone();
        let session = session_for_future.clone();
        let xterm_theme_json = xterm_theme_for_future.clone();
        let _label_dbg = label_for_future.clone();
        let mut on_session_exit = on_session_exit;
        async move {
            let host_id_js = serde_json::to_string(&host_id)
                .unwrap_or_else(|_| "\"\"".into());
            let xterm_js_json =
                serde_json::to_string(XTERM_JS).unwrap_or_default();
            let xterm_css_json =
                serde_json::to_string(XTERM_CSS).unwrap_or_default();
            let xterm_fit_js_json =
                serde_json::to_string(XTERM_FIT_JS).unwrap_or_default();
            // Mirrors the inline JS bridge in
            // shell::companion_terminal. NO outer IIFE wrapper — the
            // dioxus eval harness wraps our body in an AsyncFunction
            // and calls dioxus.close() the moment that function
            // resolves; an inner IIFE would let it resolve immediately
            // and break every subsequent Rust→JS send.
            let script = format!(
                r##"
                    const hostId = {host_id_js};
                    const showError = (msg) => {{
                        try {{ dioxus.send({{type: "error", message: msg}}); }} catch (_) {{}}
                        try {{
                            const t = document.getElementById(hostId);
                            if (t) {{
                                t.innerHTML = '';
                                const box = document.createElement("div");
                                box.style.cssText = "color:#f88;padding:1rem;font-family:ui-monospace,monospace;font-size:11px;white-space:pre-wrap;";
                                box.textContent = "Terminal failed to mount:\n\n" + msg;
                                t.appendChild(box);
                            }}
                        }} catch (_) {{}}
                    }};
                    let target = document.getElementById(hostId);
                    let attempts = 0;
                    while (!target && attempts < 60) {{
                        await new Promise(r => setTimeout(r, 33));
                        target = document.getElementById(hostId);
                        attempts++;
                    }}
                    if (!target) {{
                        try {{ dioxus.send({{type: "error", message: "host element not found id=" + hostId}}); }} catch (_) {{}}
                        return;
                    }}
                    try {{
                        if (!document.getElementById("operon-xterm-css")) {{
                            const style = document.createElement("style");
                            style.id = "operon-xterm-css";
                            style.textContent = {xterm_css_json};
                            document.head.appendChild(style);
                        }}
                        if (!window.Terminal) {{
                            const s = document.createElement("script");
                            s.textContent = {xterm_js_json};
                            document.head.appendChild(s);
                        }}
                        if (!window.FitAddon) {{
                            const s = document.createElement("script");
                            s.textContent = {xterm_fit_js_json};
                            document.head.appendChild(s);
                        }}
                    }} catch (e) {{
                        showError("xterm bootstrap failed: " + (e && e.message || e));
                        return;
                    }}
                    const FitCls = window.FitAddon && (window.FitAddon.FitAddon || window.FitAddon);
                    if (typeof window.Terminal !== "function" || typeof FitCls !== "function") {{
                        showError("xterm constructors missing after inject");
                        return;
                    }}
                    let term;
                    try {{
                        term = new window.Terminal({{
                            fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
                            fontSize: 13,
                            cursorBlink: true,
                            convertEol: false,
                            scrollback: 5000,
                            allowProposedApi: true,
                            theme: {xterm_theme_json},
                        }});
                    }} catch (e) {{
                        showError("new Terminal() threw: " + (e && e.message || e));
                        return;
                    }}
                    let fit;
                    try {{
                        fit = new FitCls();
                        term.loadAddon(fit);
                    }} catch (e) {{
                        showError("FitAddon attach threw: " + (e && e.message || e));
                        return;
                    }}
                    target.innerHTML = "";
                    try {{
                        term.open(target);
                    }} catch (e) {{
                        showError("term.open() threw: " + (e && e.message || e));
                        return;
                    }}
                    try {{ fit.fit(); }} catch (_) {{}}
                    window.__operon_terms = window.__operon_terms || {{}};
                    window.__operon_terms[hostId] = {{ term, fit }};
                    const sendSafe = (msg) => {{
                        try {{ dioxus.send(msg); }} catch (_) {{}}
                    }};
                    term.onData(d => sendSafe({{type: "data", data: d}}));
                    term.onResize(({{cols, rows}}) => sendSafe({{type: "resize", cols, rows}}));
                    const pushSize = () => {{
                        try {{ fit.fit(); }} catch (_) {{}}
                        sendSafe({{type: "resize", cols: term.cols, rows: term.rows}});
                    }};
                    pushSize();
                    if (window.ResizeObserver) {{
                        const ro = new ResizeObserver(() => pushSize());
                        ro.observe(target);
                        window.__operon_terms[hostId].ro = ro;
                    }}
                    window.addEventListener("resize", pushSize);
                    try {{ term.focus(); }} catch (_) {{}}
                    sendSafe({{type: "ready", cols: term.cols, rows: term.rows}});
                    while (true) {{
                        let incoming;
                        try {{
                            incoming = await dioxus.recv();
                        }} catch (_) {{
                            break;
                        }}
                        if (!incoming || typeof incoming !== "object") continue;
                        if (incoming.type === "out" && typeof incoming.b64 === "string") {{
                            const bin = atob(incoming.b64);
                            const bytes = new Uint8Array(bin.length);
                            for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
                            term.write(bytes);
                        }} else if (incoming.type === "reset") {{
                            term.reset();
                        }} else if (incoming.type === "theme" && incoming.theme) {{
                            try {{ term.options.theme = incoming.theme; }} catch (_) {{}}
                        }} else if (incoming.type === "shutdown") {{
                            try {{ term.dispose(); }} catch (_) {{}}
                            const entry = window.__operon_terms[hostId];
                            if (entry && entry.ro) {{
                                try {{ entry.ro.disconnect(); }} catch (_) {{}}
                            }}
                            delete window.__operon_terms[hostId];
                            return;
                        }} else if (incoming.type === "exit" && typeof incoming.message === "string") {{
                            term.write("\r\n\x1b[2m" + incoming.message + "\x1b[0m\r\n");
                        }}
                    }}
                "##,
                host_id_js = host_id_js,
                xterm_theme_json = xterm_theme_json,
            );
            let mut handle = document::eval(&script);

            // ready handshake — see companion_terminal for why this is
            // mandatory (Rust→JS sends fired before JS reaches
            // `await dioxus.recv()` are silently dropped).
            let mut out_rx = session.out_tx.subscribe();
            loop {
                match handle.recv::<serde_json::Value>().await {
                    Ok(v) => {
                        let kind =
                            v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        if kind == "ready" {
                            break;
                        }
                    }
                    Err(_) => return,
                }
            }

            // Inject SGR reset before any scrollback so the screen
            // starts in a known-clean state (no lingering bg/fg from
            // whatever the child was last writing).
            let _ = handle.send(serde_json::json!({
                "type": "out",
                "b64": B64.encode(b"\x1b[0m"),
            }));
            {
                let sb = session.scrollback.lock().unwrap();
                if !sb.is_empty() {
                    let bytes: Vec<u8> = sb.iter().copied().collect();
                    let b64 = B64.encode(&bytes);
                    let _ = handle.send(serde_json::json!({
                        "type": "out",
                        "b64": b64,
                    }));
                }
            }

            let mut child_exited = false;
            loop {
                tokio::select! {
                    biased;
                    msg = out_rx.recv() => {
                        match msg {
                            Ok(bytes) => {
                                let b64 = B64.encode(&bytes);
                                if handle
                                    .send(serde_json::json!({"type":"out","b64":b64}))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                let _ = handle.send(serde_json::json!({"type":"reset"}));
                                let _ = handle.send(serde_json::json!({
                                    "type": "out",
                                    "b64": B64.encode(b"\x1b[0m"),
                                }));
                                let sb = session.scrollback.lock().unwrap();
                                if !sb.is_empty() {
                                    let bytes: Vec<u8> = sb.iter().copied().collect();
                                    let b64 = B64.encode(&bytes);
                                    let _ = handle.send(serde_json::json!({
                                        "type": "out",
                                        "b64": b64,
                                    }));
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                // Shell exited (Ctrl-D, `exit`, killed,
                                // etc.). Tell xterm so the user sees a
                                // brief notice, then mark for auto-close.
                                let _ = handle.send(serde_json::json!({
                                    "type": "exit",
                                    "message": "shell exited.",
                                }));
                                child_exited = true;
                                break;
                            }
                        }
                    }
                    incoming = handle.recv::<serde_json::Value>() => {
                        let v = match incoming {
                            Ok(v) => v,
                            Err(_) => break,
                        };
                        let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        match kind {
                            "data" => {
                                if let Some(s) = v.get("data").and_then(|x| x.as_str()) {
                                    let bytes = s.as_bytes();
                                    if let Ok(mut w) = session.writer.lock() {
                                        let _ = w.write_all(bytes);
                                        let _ = w.flush();
                                    }
                                }
                            }
                            "resize" => {
                                let cols = v.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16;
                                let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16;
                                if let Ok(m) = session.master.lock() {
                                    let _ = m.resize(PtySize {
                                        rows,
                                        cols,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            let _ = handle.send(serde_json::json!({"type":"shutdown"}));

            // Auto-close the tab when the shell exits on its own —
            // matches a real terminal emulator where Ctrl-D dismisses
            // the tab. Skipped when we exited the loop because xterm
            // unmounted (child still alive — manager already has the
            // descriptor unless the user closed it).
            if child_exited {
                on_session_exit();
            }
        }
    });

    let (term_bg, term_fg) = match theme_kind {
        crate::theme::ThemeKind::Light => ("#ffffff", "#3b3b3b"),
        _ => ("#1e1e1e", "#d4d4d4"),
    };

    rsx! {
        div {
            class: "operon-panel-terminal",
            "data-testid": "panel-terminal",
            "data-terminal-id": "{terminal_id.0}",
            "data-cwd": "{cwd_label}",
            "data-theme": "{theme_kind.data_attr()}",
            style: "display: flex; flex-direction: column; height: 100%; min-height: 0; background: {term_bg}; color: {term_fg};",
            div {
                style: "flex: 0 0 auto; padding: 4px 8px; font-size: 11px; color: var(--vscode-descriptionforeground, var(--vscode-editor-foreground, #6e6e6e)); background: var(--vscode-panelheader-background, var(--vscode-panel-background, #f3f3f3)); border-bottom: 1px solid var(--vscode-panel-border, #e1e1e1); display: flex; gap: 12px; align-items: center;",
                span { style: "opacity: 0.85;", "shell" }
                span { style: "opacity: 0.6;", "{bin_label}" }
                span { style: "margin-left: auto; opacity: 0.6;", "cwd: {cwd_label}" }
            }
            div {
                id: "{host_id}",
                "data-testid": "panel-terminal-host",
                style: "flex: 1 1 auto; min-height: 0; padding: 6px 8px;",
            }
        }
    }
}
