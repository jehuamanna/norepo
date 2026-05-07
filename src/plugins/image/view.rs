//! Image-note pane: viewer when a blob is attached, drop / paste / picker
//! empty-state otherwise. Reads the row through the Local-Mode contexts
//! (`LocalNoteRepo`, `LocalProjectRepo`, `CurrentVaultRoot`,
//! `LocalNoteVersion`) so a successful attach bumps `note_version` and
//! the same component re-renders into the viewer branch.

#![cfg(not(target_arch = "wasm32"))]

use dioxus::html::HasFileData;
use dioxus::prelude::*;
use operon_store::repos::NoteKind;
use uuid::Uuid;

use crate::local_mode::desktop::{CurrentVaultRoot, LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::LocalNoteVersion;
use crate::local_mode::images::{self, extension_for_mime};
use crate::local_mode::vault::VaultRoot;

/// Single entry-point for both View and Edit. `editable=false` hides the
/// drop / paste / picker affordances when the row has no blob attached.
#[component]
pub fn ImageNotePane(note_id: String, editable: bool) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let LocalProjectRepo(project_repo) = use_context();
    let CurrentVaultRoot(vault_signal) = use_context();
    // Subscribe to note_version so that an attach (drop / paste / picker)
    // re-renders this pane and we re-read the row's `blob_path`. Without
    // the explicit `.read()` call Dioxus doesn't track the dependency
    // and the empty-state stays stuck after the first attach.
    let LocalNoteVersion(note_version) = use_context::<LocalNoteVersion>();
    let _ = note_version.read();

    let Ok(note_uuid) = Uuid::parse_str(&note_id) else {
        return rsx! {
            div { class: "operon-main-empty", "Invalid image note id." }
        };
    };

    let row = {
        let projects = match project_repo.list() {
            Ok(p) => p,
            Err(e) => {
                return rsx! {
                    div {
                        class: "operon-main-empty",
                        "Couldn't load projects: {e}"
                    }
                };
            }
        };
        projects.into_iter().find_map(|p| {
            note_repo
                .list_for_project(p.id)
                .ok()?
                .into_iter()
                .find(|n| n.id == note_uuid)
        })
    };
    let Some(row) = row else {
        return rsx! {
            div { class: "operon-main-empty", "Image note not found." }
        };
    };
    if !matches!(row.kind, NoteKind::Image) {
        return rsx! {
            div {
                class: "operon-main-empty",
                "Note kind is not Image."
            }
        };
    }

    let vault = vault_signal.read().clone();

    // Blob attached: render the viewer.
    if let Some(rel) = row.blob_path.clone() {
        let Some(vault) = vault else {
            return rsx! {
                div { class: "operon-main-empty", "Vault is not open." }
            };
        };
        let rel_path = std::path::PathBuf::from(&rel);
        let Some(data_url) = images::data_url_for_blob(&vault, &rel_path) else {
            return rsx! {
                div {
                    class: "operon-main-empty",
                    "Image blob is missing on disk: {rel}"
                }
            };
        };

        // Plans-Phase-2-editor-auto-focus: keep the viewer focusable so
        // arrow keys / page-up/down scroll after a tab open.
        let crate::editor::RequestEditorFocus(mut focus_request) = use_context();
        let note_id_for_focus = note_uuid.to_string();
        let title = row.title.clone();
        return rsx! {
            div {
                class: "operon-local-image-view",
                "data-testid": "image-note-view",
                "data-note-id": "{note_uuid}",
                tabindex: "-1",
                onmounted: move |evt| {
                    let wants_focus = focus_request
                        .read()
                        .as_deref()
                        .map(|id| id == note_id_for_focus.as_str())
                        .unwrap_or(false);
                    if wants_focus {
                        drop(evt.set_focus(true));
                        focus_request.set(None);
                    }
                },
                style: "display: flex; align-items: center; justify-content: center; height: 100%; overflow: auto; padding: 1rem; background: var(--vscode-editor-background); color: var(--vscode-editor-foreground);",
                img {
                    src: "{data_url}",
                    alt: "{title}",
                    style: "max-width: 100%; max-height: 100%; object-fit: contain;",
                }
            }
        };
    }

    // Empty-state. View mode renders a passive message; Edit mode wires
    // up drop / paste / picker.
    if !editable {
        return rsx! {
            div {
                class: "operon-local-image-empty operon-local-image-empty-readonly",
                "data-testid": "image-note-empty-readonly",
                "data-note-id": "{note_uuid}",
                style: "display: flex; align-items: center; justify-content: center; height: 100%; padding: 2rem; text-align: center; color: var(--vscode-editor-foreground); opacity: 0.7; background: var(--vscode-editor-background);",
                "No image attached yet."
            }
        };
    }

    rsx! { ImageNoteEmptyState { note_id: note_uuid } }
}

/// Edit-mode empty-state for an image note: drop area, paste support,
/// and an explicit "Choose image…" file picker. Any of the three writes
/// the bytes via `images::write_image`, stamps `blob_path` on the row,
/// and bumps `note_version` so the parent pane re-renders into the
/// viewer branch.
#[component]
fn ImageNoteEmptyState(note_id: Uuid) -> Element {
    let LocalNoteRepo(note_repo) = use_context();
    let CurrentVaultRoot(vault_signal) = use_context();
    let LocalNoteVersion(note_version) = use_context::<LocalNoteVersion>();

    let mut drag_active: Signal<bool> = use_signal(|| false);
    let error: Signal<Option<String>> = use_signal(|| None);

    // Clipboard paste pipeline. Two paths fan into the same Rust loop:
    //   1. JS keydown bridge: a document-level Ctrl/Cmd+V listener fires
    //      `dioxus.send({ request: "paste" })`; Rust then reads the OS
    //      clipboard via `arboard` and encodes RGBA → PNG. This avoids the
    //      `navigator.clipboard.read()` permission gate that WebKitGTK/wry
    //      rejects with `NotAllowedError`.
    //   2. JS paste-event fallback: only fires when an editable element
    //      is focused (inline rename, etc.) — uses the legacy
    //      `e.clipboardData.items` path with file blobs and base64 over
    //      the bridge. Slightly redundant with path 1, but free and
    //      handles the case where Ctrl+V was already consumed by the
    //      browser's default paste behavior.
    {
        let note_repo = note_repo.clone();
        let vault_signal = vault_signal;
        let mut note_version = note_version;
        let mut error = error;
        let target_note = note_id;
        use_future(move || {
            let note_repo = note_repo.clone();
            async move {
                // Idempotent listener install. Hot-reloads / remounts re-run
                // this eval string, but the guard flag (`__operonImagePasteWired`)
                // means handlers are only registered once per webview. Each
                // run *does* update `__operonImagePasteSend.current` so events
                // route into the freshest eval channel — old eval handles
                // become no-ops once their successor takes over.
                // Stack-of-dispatchers pattern (see `local_mode/editor/mod
                // .rs` for the long form): a single `.current` pointer
                // would go stale every time an earlier empty-state's
                // eval was dropped, and the next paste would throw
                // `null is not an object (window.getQuery(N).rustSend)`.
                // Instead each empty-state mount pushes its own send
                // onto a stack; the dispatcher tries the latest with a
                // try/catch fallback, popping dead entries lazily.
                let mut eval = document::eval(
                    "if (!window.__operonImagePasteWired) { \
                        window.__operonImagePasteSenders = []; \
                        window.__operonImagePasteSend = function(payload) { \
                            const list = window.__operonImagePasteSenders; \
                            while (list.length > 0) { \
                                const fn = list[list.length - 1]; \
                                try { fn(payload); return; } \
                                catch (e) { list.pop(); } \
                            } \
                            console.warn('operon: no live image-paste dispatcher'); \
                        }; \
                        document.addEventListener('keydown', function(e) { \
                            if (!((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey)) return; \
                            if (e.key !== 'v' && e.key !== 'V') return; \
                            if (!document.querySelector('[data-testid=\"image-note-empty\"]')) return; \
                            e.preventDefault(); \
                            e.stopPropagation(); \
                            window.__operonImagePasteSend({ request: 'paste' }); \
                        }, true); \
                        document.addEventListener('paste', async function(e) { \
                            if (!e.clipboardData) return; \
                            for (const item of e.clipboardData.items) { \
                                if (item.kind === 'file' && item.type && item.type.startsWith('image/')) { \
                                    const blob = item.getAsFile(); \
                                    if (!blob) continue; \
                                    try { \
                                        const buf = await blob.arrayBuffer(); \
                                        const u8 = new Uint8Array(buf); \
                                        let bin = ''; \
                                        for (let i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i]); \
                                        const b64 = btoa(bin); \
                                        e.preventDefault(); \
                                        window.__operonImagePasteSend({ mime: item.type, name: blob.name || 'pasted', b64 }); \
                                    } catch (err) { \
                                        window.__operonImagePasteSend({ error: '[js-paste] Decode failed: ' + (err && err.message ? err.message : err) }); \
                                    } \
                                    return; \
                                } \
                            } \
                        }); \
                        window.__operonImagePasteWired = true; \
                    } \
                    window.__operonImagePasteSenders.push(function(payload) { dioxus.send(payload); });",
                );
                loop {
                    let msg: serde_json::Value = match eval.recv().await {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                    if let Some(err_msg) = msg.get("error").and_then(|v| v.as_str()) {
                        error.set(Some(err_msg.to_string()));
                        continue;
                    }
                    // Path 1: JS keydown bridge → Rust arboard read.
                    if msg.get("request").and_then(|v| v.as_str()) == Some("paste") {
                        let (bytes, mime) = match crate::util::clipboard::read_clipboard_image_png() {
                            Ok(pair) => pair,
                            Err(e) => {
                                error.set(Some(e));
                                continue;
                            }
                        };
                        let Some(vault) = vault_signal.read().clone() else {
                            error.set(Some("Vault is not open.".into()));
                            continue;
                        };
                        attach_blob(
                            target_note,
                            "pasted",
                            mime,
                            bytes,
                            &vault,
                            &note_repo,
                            &mut note_version,
                            &mut error,
                        );
                        continue;
                    }
                    // Path 2: JS paste-event fallback (already-encoded blob bytes).
                    let mime = msg.get("mime").and_then(|v| v.as_str()).unwrap_or("");
                    let name = msg
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("pasted")
                        .to_string();
                    let b64 = msg.get("b64").and_then(|v| v.as_str()).unwrap_or("");
                    let bytes = match base64_decode(b64) {
                        Some(b) => b,
                        None => {
                            error.set(Some("Failed to decode pasted image.".into()));
                            continue;
                        }
                    };
                    let Some(vault) = vault_signal.read().clone() else {
                        error.set(Some("Vault is not open.".into()));
                        continue;
                    };
                    attach_blob(
                        target_note,
                        &name,
                        mime,
                        bytes,
                        &vault,
                        &note_repo,
                        &mut note_version,
                        &mut error,
                    );
                }
            }
        });
    }

    let pick_image = {
        let note_repo = note_repo.clone();
        let vault_signal = vault_signal;
        let mut note_version = note_version;
        let mut error = error;
        move || {
            let note_repo = note_repo.clone();
            spawn(async move {
                let Some(handle) = rfd::AsyncFileDialog::new()
                    .set_title("Choose an image")
                    .add_filter(
                        "Image",
                        &["png", "jpg", "jpeg", "webp", "gif", "svg", "avif"],
                    )
                    .pick_file()
                    .await
                else {
                    return;
                };
                let path = handle.path().to_path_buf();
                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(e) => {
                        error.set(Some(format!("Could not read file: {e}")));
                        return;
                    }
                };
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                let mime = match ext.as_str() {
                    "png" => "image/png",
                    "jpg" | "jpeg" => "image/jpeg",
                    "webp" => "image/webp",
                    "gif" => "image/gif",
                    "svg" => "image/svg+xml",
                    "avif" => "image/avif",
                    _ => {
                        error.set(Some(format!("Unsupported image extension: .{ext}")));
                        return;
                    }
                };
                let Some(vault) = vault_signal.read().clone() else {
                    error.set(Some("Vault is not open.".into()));
                    return;
                };
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("picked")
                    .to_string();
                attach_blob(
                    note_id,
                    &name,
                    mime,
                    bytes,
                    &vault,
                    &note_repo,
                    &mut note_version,
                    &mut error,
                );
            });
        }
    };

    // Click-triggered paste path. Uses the same Rust-side `arboard` read as
    // the Ctrl+V keydown bridge, so it works regardless of whether
    // document-level keydown events are intercepted by Monaco or any other
    // listener. This is the always-works fallback for the empty-state. The
    // body is spawned (matching `pick_image`) so the outer closure stays
    // `Fn` for use as an `onclick` handler.
    let paste_from_clipboard = {
        let note_repo = note_repo.clone();
        let vault_signal = vault_signal;
        let mut note_version = note_version;
        let mut error = error;
        move || {
            let note_repo = note_repo.clone();
            spawn(async move {
                let (bytes, mime) = match crate::util::clipboard::read_clipboard_image_png() {
                    Ok(p) => p,
                    Err(e) => {
                        error.set(Some(e));
                        return;
                    }
                };
                let Some(vault) = vault_signal.read().clone() else {
                    error.set(Some("Vault is not open.".into()));
                    return;
                };
                attach_blob(
                    note_id,
                    "pasted",
                    mime,
                    bytes,
                    &vault,
                    &note_repo,
                    &mut note_version,
                    &mut error,
                );
            });
        }
    };

    let on_drop = {
        let note_repo = note_repo.clone();
        let vault_signal = vault_signal;
        let note_version = note_version;
        let mut error = error;
        let mut drag_active = drag_active;
        move |evt: Event<DragData>| {
            evt.prevent_default();
            drag_active.set(false);
            let files = evt.data().files();
            let Some(first) = files.into_iter().next() else {
                return;
            };
            let name = first.name();
            let lower = name.to_ascii_lowercase();
            let mime = if lower.ends_with(".png") {
                "image/png"
            } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                "image/jpeg"
            } else if lower.ends_with(".webp") {
                "image/webp"
            } else if lower.ends_with(".gif") {
                "image/gif"
            } else if lower.ends_with(".svg") {
                "image/svg+xml"
            } else if lower.ends_with(".avif") {
                "image/avif"
            } else {
                error.set(Some(format!("Unsupported image file: {name}")));
                return;
            };
            let note_repo = note_repo.clone();
            let vault_signal = vault_signal;
            let mut note_version = note_version;
            let mut error = error;
            spawn(async move {
                let bytes = match first.read_bytes().await {
                    Ok(b) => b.to_vec(),
                    Err(e) => {
                        error.set(Some(format!("Could not read drop: {e:?}")));
                        return;
                    }
                };
                let Some(vault) = vault_signal.read().clone() else {
                    error.set(Some("Vault is not open.".into()));
                    return;
                };
                attach_blob(
                    note_id,
                    &name,
                    mime,
                    bytes,
                    &vault,
                    &note_repo,
                    &mut note_version,
                    &mut error,
                );
            });
        }
    };

    let highlight = *drag_active.read();
    let base_style = "display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.75rem; height: 100%; padding: 2rem; text-align: center; color: var(--vscode-editor-foreground); background: var(--vscode-editor-background); transition: border-color 120ms ease-in-out, background-color 120ms ease-in-out;";
    let dropzone_style = if highlight {
        format!(
            "{base_style} border: 2px dashed var(--vscode-focusborder, #0078D4); background: color-mix(in srgb, var(--vscode-focusborder, #0078D4) 8%, var(--vscode-editor-background));"
        )
    } else {
        format!(
            "{base_style} border: 2px dashed var(--vscode-panel-border, var(--vscode-editorwidget-border, rgba(127, 127, 127, 0.35)));"
        )
    };

    rsx! {
        div {
            class: "operon-local-image-empty",
            "data-testid": "image-note-empty",
            "data-note-id": "{note_id}",
            "data-drag-active": if highlight { "true" } else { "false" },
            style: "{dropzone_style}",
            ondragenter: move |evt| {
                evt.prevent_default();
                drag_active.set(true);
            },
            ondragover: move |evt| {
                evt.prevent_default();
                if !*drag_active.peek() {
                    drag_active.set(true);
                }
            },
            ondragleave: move |evt| {
                evt.prevent_default();
                drag_active.set(false);
            },
            ondrop: on_drop,
            div {
                style: "font-size: 2rem; opacity: 0.5;",
                "\u{1F5BC}"
            }
            p {
                style: "margin: 0; font-weight: 500;",
                "Drop an image here, paste from clipboard, or pick a file"
            }
            div {
                style: "display: flex; gap: 0.5rem; flex-wrap: wrap; justify-content: center;",
                button {
                    r#type: "button",
                    class: "operon-button",
                    "data-testid": "image-note-pick-button",
                    style: "padding: 0.4rem 0.9rem; border-radius: 0.25rem; cursor: pointer; border: 1px solid var(--vscode-button-border, transparent); background: var(--vscode-button-background); color: var(--vscode-button-foreground); font: inherit;",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        pick_image();
                    },
                    "Choose image\u{2026}"
                }
                button {
                    r#type: "button",
                    class: "operon-button",
                    "data-testid": "image-note-paste-button",
                    style: "padding: 0.4rem 0.9rem; border-radius: 0.25rem; cursor: pointer; border: 1px solid var(--vscode-button-border, transparent); background: var(--vscode-button-secondarybackground, var(--vscode-button-background)); color: var(--vscode-button-foreground); font: inherit;",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        paste_from_clipboard();
                    },
                    "Paste from clipboard"
                }
            }
            p {
                style: "margin: 0; font-size: 0.85em; opacity: 0.55;",
                "PNG, JPG, WebP, GIF, SVG, AVIF \u{2014} max 25 MB"
            }
            if let Some(err) = error.read().clone() {
                p {
                    "data-testid": "image-note-error",
                    style: "margin: 0; font-size: 0.85em; color: var(--vscode-errorforeground, #f48771);",
                    "{err}"
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn attach_blob(
    note_id: Uuid,
    filename: &str,
    mime: &str,
    bytes: Vec<u8>,
    vault: &VaultRoot,
    note_repo: &std::sync::Arc<dyn operon_store::repos::LocalNoteRepository>,
    note_version: &mut Signal<u64>,
    error: &mut Signal<Option<String>>,
) {
    if extension_for_mime(mime).is_none() {
        error.set(Some(format!("Unsupported MIME type: {mime}")));
        return;
    }
    let written = match images::write_image(vault, &bytes, mime) {
        Ok(w) => w,
        Err(e) => {
            error.set(Some(format!("Failed to save image: {e}")));
            return;
        }
    };
    let _ = filename;
    let rel = written.relative_path.to_string_lossy().to_string();
    if let Err(e) = note_repo.set_blob_path(note_id, Some(&rel)) {
        error.set(Some(format!("Failed to attach blob: {e}")));
        return;
    }
    note_version.with_mut(|v| *v += 1);
    error.set(None);
}

/// Tiny inline base64 decoder matching the one in
/// `local_mode::editor::base64_decode`. Standard alphabet, ignores
/// whitespace, tolerates missing padding.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.chars() {
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '+' => 62,
            '/' => 63,
            '=' | ' ' | '\n' | '\r' | '\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}
