//! Per-tab mode toolbar — View / Edit / Live Preview / Split.
//!
//! Renders one button per editor mode the active tab's `FormatPlugin` claims via its
//! `capabilities()`. Modes the plugin doesn't claim are hidden — never offered. Active mode
//! gets `aria-pressed="true"` plus an active CSS class.
//!
//! Lives directly above the `MainArea` body; clicking a button mutates the tab's
//! `mode: EditorMode` field which the dispatcher reads on the next render.
//!
//! **Revision flow integration.** For every EDIT-capable format
//! *except* skill (whose own toolbar embeds `RevisionFlowButtons`
//! next to its ▶ Run button), this toolbar mounts a
//! `RevisionFlowButtons` cluster. It supersedes the plain `Revise`
//! mode button — clicking the cluster's "Edit" snapshots the body,
//! flips to Edit, and reveals Cancel/Done. Done opens a required
//! manual-summary dialog and appends a `manual` row to the body's
//! `## Revision history` table.

use std::rc::Rc;

use dioxus::prelude::*;

use crate::editor::EditorMode;
use crate::plugin::{FormatCaps, PluginRegistry};
use crate::tabs::TabManager;

#[component]
pub fn ModeToolbar() -> Element {
    let tabs: Signal<TabManager> = use_context();
    let registry: Rc<PluginRegistry> = use_context();

    let active = {
        let snapshot = tabs.read();
        snapshot
            .active()
            .map(|t| (t.id, t.format_id.clone(), t.note_id.clone(), t.title.clone(), t.mode))
    };

    let Some((tab_id, format_id, note_id, note_title, mode)) = active else {
        return rsx! { div { class: "operon-mode-toolbar operon-mode-toolbar-empty" } };
    };
    // `note_id` is only consumed by the desktop `RevisionFlowButtons`
    // branch; suppress the unused-binding warning on wasm.
    #[cfg(target_arch = "wasm32")]
    let _ = &note_id;
    let caps = registry
        .format_plugin_for(&format_id)
        .map(|p| p.capabilities())
        .unwrap_or(FormatCaps::NONE);

    // Skill mounts the revise/done/cancel cluster inline alongside
    // its own ▶ Run button (see `plugins::skill::view::SkillToolbar`),
    // so the mode toolbar emits NO Revise button for that format —
    // otherwise we'd have two competing entry points into Edit, and
    // the mode-toolbar one wouldn't snapshot `prior_body` so Cancel
    // would have nothing to revert to.
    let edit_capable = caps.contains(FormatCaps::EDIT);
    let revise_cluster: Element = build_revise_cluster(
        edit_capable,
        format_id == "skill",
        note_id.clone(),
        tab_id,
        mode,
        tabs,
    );

    // The Send-to-Claude button writes a `@[<title>](note:<uuid>)`
    // mention token to `CompanionComposerAppend` — same shape and same
    // consumer the explorer's right-click action uses, so the chat
    // composer's chip tray + send-time prompt rewriter see no new
    // payload. Only offered on body-bearing formats (gated on EDIT
    // capability), since image / canvas / etc. don't have a markdown
    // body Claude can usefully read.
    let send_button: Element = build_send_to_claude_cluster(
        caps.contains(FormatCaps::EDIT),
        note_id.clone(),
        note_title.clone(),
    );

    rsx! {
        div { class: "operon-mode-toolbar",
            "data-component": "mode-toolbar",
            "data-active-mode": mode_slug(mode),
            if caps.contains(FormatCaps::VIEW) {
                ModeButton { mode_label: "View", target: EditorMode::View, current: mode, tab_id, tabs }
            }
            {revise_cluster}
            if caps.contains(FormatCaps::LIVE_PREVIEW) {
                ModeButton { mode_label: "Live Preview", target: EditorMode::LivePreview, current: mode, tab_id, tabs }
            }
            if caps.contains(FormatCaps::VIEW) && caps.contains(FormatCaps::EDIT) {
                // Split is a shell layout that pairs view+edit; only offered when both are
                // available. Phase 3 implements the actual paired-pane rendering.
                ModeButton { mode_label: "Split", target: EditorMode::Split, current: mode, tab_id, tabs }
            }
            {send_button}
        }
    }
}

/// Build the "Send to Claude" toolbar button for body-bearing formats.
/// Returns an empty fragment when the format isn't EDIT-capable (e.g.
/// image, canvas) or when the companion isn't mounted (no
/// `CompanionComposerAppend` context — tests, vault-less standalone).
fn build_send_to_claude_cluster(
    edit_capable: bool,
    note_id: String,
    note_title: String,
) -> Element {
    if !edit_capable {
        let _ = (note_id, note_title);
        return rsx! {};
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let append_handle =
            try_consume_context::<crate::shell::companion_state::CompanionComposerAppend>()
                .map(|c| c.0);
        let Some(mut append_sig) = append_handle else {
            let _ = (note_id, note_title);
            return rsx! {};
        };
        // Captures for each onclick. Signal is Copy so duplicating
        // the handle for both buttons is cheap.
        let note_id_for_send = note_id.clone();
        let note_title_for_send = note_title.clone();
        let mut append_sig_for_send = append_sig;
        let note_id_for_sel = note_id.clone();
        let note_title_for_sel = note_title.clone();
        let mut append_sig_for_sel = append_sig;
        rsx! {
            button {
                class: "operon-mode-button",
                r#type: "button",
                "data-testid": "mode-send-to-claude",
                title: "Send this note to the companion (Claude). In chat mode it lands as a chip in the composer; in terminal mode the mention is typed at the prompt.",
                onclick: move |_| {
                    let token = format!("@[{}](note:{})", note_title_for_send, note_id_for_send);
                    // Always feed the chat composer signal — the
                    // chip stays put if the user later switches to
                    // chat mode and the gesture isn't lost.
                    append_sig_for_send.set(Some(token.clone()));
                    // M4d.1: also feed the terminal-injection signal
                    // with a trailing space so the cursor lands past
                    // the mention. The currently-mounted
                    // `ClaudeRepoTerminal` (if any) drains this on
                    // its next render and types the token at the
                    // claude prompt. If the user is in chat mode,
                    // there's no terminal to drain — the value sits
                    // until they mount one or until it's overwritten
                    // by a later send.
                    *crate::shell::companion_state::PENDING_TERMINAL_INJECTION.write() =
                        Some(format!("{token} "));
                    // Bump the expand tick so a collapsed companion
                    // pops open — without this, clicking the button
                    // with the pane collapsed silently queues the
                    // chip and the user gets no visible feedback.
                    let cur = *crate::shell::companion_state::EXPAND_COMPANION_TICK.peek();
                    *crate::shell::companion_state::EXPAND_COMPANION_TICK.write() =
                        cur.wrapping_add(1);
                },
                "Send to Claude"
            }
            // M4d-selection: send a focused line-range hint to
            // Claude based on whatever the user has highlighted in
            // the active Monaco editor right now. JS reads
            // `window.__operon_active_monaco_id` (set by editor_host
            // on focus) → snapshots that handle → reports the
            // selection back via `dioxus.send`. The Rust click
            // handler then composes a mention + range payload and
            // writes it through the same signals as Send-to-Claude.
            // No-op if there's no active editor or no selection.
            button {
                class: "operon-mode-button",
                r#type: "button",
                "data-testid": "mode-send-selection",
                title: "Send a focus hint for whatever you've highlighted in this editor. Claude will fetch the note with get_note and look at those lines.",
                onclick: move |_| {
                    let note_id = note_id_for_sel.clone();
                    let note_title = note_title_for_sel.clone();
                    let mut handle = document::eval(
                        r#"(async function() {
                            try {
                                const id = window.__operon_active_monaco_id;
                                const h = id && (window.__operon_monaco_handles || {})[id];
                                if (!h) { dioxus.send({ok: false, reason: 'no active editor'}); return; }
                                const snap = h.snapshot();
                                if (!snap || !snap.selection) { dioxus.send({ok: false, reason: 'no selection'}); return; }
                                const s = snap.selection[0];
                                const e = snap.selection[1];
                                if (s === e) { dioxus.send({ok: false, reason: 'empty selection'}); return; }
                                const content = h.getContent();
                                const before = content.slice(0, s);
                                const sel = content.slice(s, e);
                                const startLine = (before.match(/\n/g) || []).length + 1;
                                const endLine = startLine + (sel.match(/\n/g) || []).length;
                                dioxus.send({
                                    ok: true,
                                    start_line: startLine,
                                    end_line: endLine,
                                    chars: e - s,
                                });
                            } catch (err) {
                                dioxus.send({ok: false, reason: String(err && err.message || err)});
                            }
                        })();"#,
                    );
                    spawn(async move {
                        let reply: serde_json::Value = match handle.recv().await {
                            Ok(v) => v,
                            Err(_) => return,
                        };
                        if !reply.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                            return;
                        }
                        let start = reply
                            .get("start_line")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let end = reply.get("end_line").and_then(|v| v.as_u64()).unwrap_or(0);
                        let chars =
                            reply.get("chars").and_then(|v| v.as_u64()).unwrap_or(0);
                        let range = if start == end {
                            format!("line {start}")
                        } else {
                            format!("lines {start}–{end}")
                        };
                        // Single-line, fence-free payload so it
                        // works equally well as a chat-mode chip
                        // companion (the chip captures the mention;
                        // the trailing prose lands as composer
                        // remainder via the multi-mention consumer
                        // in `companion_chat.rs`) AND as a
                        // terminal-mode PTY type-in (no awkward
                        // multi-line bracketing).
                        let payload = format!(
                            "@[{note_title}](note:{note_id}) — focus on {range} ({chars} chars selected)"
                        );
                        append_sig_for_sel.set(Some(payload.clone()));
                        *crate::shell::companion_state::PENDING_TERMINAL_INJECTION.write() =
                            Some(format!("{payload} "));
                        let cur = *crate::shell::companion_state::EXPAND_COMPANION_TICK.peek();
                        *crate::shell::companion_state::EXPAND_COMPANION_TICK.write() =
                            cur.wrapping_add(1);
                    });
                },
                "Send selection"
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        // No companion on the web build yet; surface nothing rather
        // than a dead button.
        let _ = (note_id, note_title);
        rsx! {}
    }
}

/// Build the Edit-mode button(s) the toolbar shows for an EDIT-capable
/// format. On desktop, non-skill formats get the rich
/// `RevisionFlowButtons` cluster (Revise + Cancel + Done dialog). Skill
/// + wasm fall back to a plain `Revise` mode button — skill because
/// its own toolbar mounts the cluster; wasm because `Persistence` save
/// isn't wired through the OPFS backend yet.
fn build_revise_cluster(
    edit_capable: bool,
    owns_inline_revise: bool,
    note_id: String,
    tab_id: crate::tabs::TabId,
    mode: EditorMode,
    tabs: Signal<TabManager>,
) -> Element {
    if !edit_capable {
        let _ = (note_id, tab_id, mode, tabs);
        return rsx! {};
    }
    if owns_inline_revise {
        // Skill: its own toolbar mounts the cluster. Emit nothing
        // here so the user has exactly one Edit entry point and so
        // `prior_body` is always snapshotted on the View→Edit flip.
        let _ = (note_id, tab_id, mode, tabs);
        return rsx! {};
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (tab_id, mode, tabs);
        rsx! {
            crate::plugins::revise_flow::RevisionFlowButtons {
                note_id,
                class_root: "operon-mode-button-revise".to_string(),
                testid_prefix: "mode-revise".to_string(),
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Web: `Persistence` save isn't wired through OPFS yet; fall
        // back to the plain mode button until that lands.
        let _ = note_id;
        rsx! {
            ModeButton { mode_label: "Revise", target: EditorMode::Edit, current: mode, tab_id, tabs }
        }
    }
}

#[component]
fn ModeButton(
    mode_label: &'static str,
    target: EditorMode,
    current: EditorMode,
    tab_id: crate::tabs::TabId,
    tabs: Signal<TabManager>,
) -> Element {
    let active = current == target;
    let class = if active {
        "operon-mode-button operon-mode-button-active"
    } else {
        "operon-mode-button"
    };
    let mut tabs = tabs;
    rsx! {
        button {
            class: "{class}",
            r#type: "button",
            "data-mode": mode_slug(target),
            "aria-pressed": if active { "true" } else { "false" },
            onclick: move |_| {
                tabs.write().set_mode(tab_id, target);
            },
            "{mode_label}"
        }
    }
}

const fn mode_slug(m: EditorMode) -> &'static str {
    match m {
        EditorMode::View => "view",
        EditorMode::Edit => "edit",
        EditorMode::LivePreview => "live-preview",
        EditorMode::Split => "split",
    }
}
